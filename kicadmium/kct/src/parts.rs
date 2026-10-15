//! LCSC/JLCPCB component catalog client and persistent cache.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SEARCH: &str =
    "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList/v2";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    pub quantity: u64,
    pub unit_price: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    /// Catalog supplying this record; old cache entries came from JLCPCB.
    #[serde(default = "jlcpcb_source")]
    pub source: String,
    /// Stock is specific to this distributor, not interchangeable assembly stock.
    #[serde(default = "jlcpcb_source")]
    pub stock_source: String,
    pub lcsc_part: String,
    pub mfr_part: String,
    pub manufacturer: String,
    pub description: String,
    pub package: String,
    pub stock: i64,
    pub min_order: u64,
    pub prices: Vec<Price>,
    pub is_basic: bool,
    pub datasheet_url: String,
    pub product_url: String,
    pub fetched_at: u64,
}
fn jlcpcb_source() -> String {
    "jlcpcb".into()
}
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub parts: Vec<Part>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
    pub source: &'static str,
}
fn client() -> Result<reqwest::blocking::Client> {
    crate::install_tls_provider();
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("kicadmium-kct/", env!("CARGO_PKG_VERSION")))
        .build()?)
}
fn post(url: &str, payload: Value) -> Result<Value> {
    let response = client()?
        .post(url)
        .header("Origin", "https://jlcpcb.com")
        .header("Referer", "https://jlcpcb.com/parts")
        .json(&payload)
        .send()
        .with_context(|| format!("parts capability unavailable: POST {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("parts capability unavailable: POST {url} returned HTTP {status}")
    }
    response
        .json()
        .with_context(|| format!("invalid JLCPCB parts API response from {url}"))
}
pub fn normalize_part(part: &str) -> String {
    let p = part.trim().to_ascii_uppercase();
    if p.starts_with('C') {
        p
    } else {
        format!("C{p}")
    }
}
pub fn lookup(part: &str) -> Result<Option<Part>> {
    let id = normalize_part(part);
    anyhow::ensure!(valid_id(&id), "invalid LCSC part number: {part}");
    match lookup_jlcpcb(&id) {
        Ok(Some(part)) => Ok(Some(part)),
        primary => lookup_easyeda(&id).with_context(|| match primary {
            Err(error) => format!("JLCPCB lookup failed: {error:#}; EasyEDA fallback failed"),
            _ => "JLCPCB had no record; EasyEDA fallback failed".into(),
        }),
    }
}
fn valid_id(id: &str) -> bool {
    id.starts_with('C') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit())
}
fn lookup_jlcpcb(id: &str) -> Result<Option<Part>> {
    let root = post(
        SEARCH,
        serde_json::json!({"keyword":id,"pageSize":100,"currentPage":1}),
    )?;
    if root.get("code").and_then(Value::as_i64) != Some(200) {
        bail!(
            "parts capability unavailable: {SEARCH} returned business error: {}",
            root.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )
    }
    let rows = root
        .pointer("/data/componentPageInfo/list")
        .and_then(Value::as_array)
        .context("JLCPCB lookup response missing componentPageInfo/list")?;
    rows.iter()
        .find(|row| row["componentCode"].as_str() == Some(id))
        .map(parse_part)
        .transpose()
}
pub fn search(query: &str, page_size: u64, in_stock: bool, basic: bool) -> Result<SearchResult> {
    let id = normalize_part(query);
    if valid_id(&id) {
        let parts: Vec<_> = lookup(&id)?
            .into_iter()
            .filter(|p| (!in_stock || p.stock > 0) && (!basic || p.is_basic))
            .take(page_size.min(100) as usize)
            .collect();
        return Ok(SearchResult {
            total: parts.len() as u64,
            page: 1,
            page_size,
            parts,
            source: "exact-part lookup",
        });
    }
    let mut payload =
        serde_json::json!({"keyword":query,"pageSize":page_size.min(100),"currentPage":1});
    if in_stock {
        payload["stockCountMin"] = 1.into()
    }
    if basic {
        payload["componentLibraryType"] = "base".into()
    }
    let root = post(SEARCH, payload).context(
        "keyword search unavailable; exact LCSC-number search/lookup supports the EasyEDA fallback",
    )?;
    if root.get("code").and_then(Value::as_i64) != Some(200) {
        bail!(
            "parts capability unavailable: {SEARCH} returned code {}: {}",
            root["code"],
            root["message"]
        )
    }
    let info = root
        .pointer("/data/componentPageInfo")
        .context("response missing componentPageInfo")?;
    let rows = info
        .get("list")
        .or_else(|| info.get("content"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let parts = rows.iter().map(parse_part).collect::<Result<Vec<_>>>()?;
    Ok(SearchResult {
        total: info
            .get("total")
            .or_else(|| info.get("totalElements"))
            .and_then(Value::as_u64)
            .unwrap_or(parts.len() as u64),
        page: 1,
        page_size,
        parts,
        source: "jlcpcb",
    })
}
fn parse_part(v: &Value) -> Result<Part> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let prices = v
        .get("prices")
        .or_else(|| v.get("componentPrices"))
        .or_else(|| v.get("priceList"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(Price {
                quantity: p.get("startNumber")?.as_u64()?,
                unit_price: p
                    .get("productPrice")?
                    .as_f64()
                    .or_else(|| p.get("productPrice")?.as_str()?.parse().ok())?,
            })
        })
        .collect();
    let lcsc = s("componentCode");
    anyhow::ensure!(
        valid_id(&lcsc),
        "JLCPCB response has no valid componentCode"
    );
    Ok(Part {
        source: "jlcpcb".into(),
        stock_source: "jlcpcb".into(),
        lcsc_part: lcsc.clone(),
        mfr_part: {
            let x = s("componentModelEn");
            if x.is_empty() {
                s("manufacturerPartNumber")
            } else {
                x
            }
        },
        manufacturer: {
            let x = s("componentBrandEn");
            if x.is_empty() {
                s("manufacturer")
            } else {
                x
            }
        },
        description: {
            let x = s("describe");
            if x.is_empty() {
                s("componentModelEn")
            } else {
                x
            }
        },
        package: {
            let x = s("encapStandard");
            if x.is_empty() {
                let package = s("componentSpecificationEn");
                if package.is_empty() {
                    s("package")
                } else {
                    package
                }
            } else {
                x
            }
        },
        stock: v.get("stockCount").and_then(Value::as_i64).unwrap_or(-1),
        min_order: v
            .get("minOrder")
            .or_else(|| v.get("minPurchaseNum"))
            .and_then(Value::as_u64)
            .unwrap_or(1),
        prices,
        is_basic: v.get("componentLibraryType").and_then(Value::as_str) == Some("base"),
        datasheet_url: s("dataManualUrl"),
        product_url: format!("https://jlcpcb.com/partdetail/{lcsc}"),
        fetched_at: now(),
    })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
pub fn cache_dir() -> PathBuf {
    std::env::var_os("KICADMIUM_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache/kicadmium")))
        .unwrap_or_else(|| PathBuf::from(".kicadmium"))
        .join("parts")
}
pub fn cache_put(part: &Part) -> Result<()> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    crate::fsutil::atomic_write(
        &dir.join(format!("{}.json", part.lcsc_part)),
        serde_json::to_vec_pretty(part)?.as_slice(),
    )
}
pub fn cache_get(id: &str, max_age_days: u64) -> Result<Option<Part>> {
    let path = cache_dir().join(format!("{}.json", normalize_part(id)));
    if !path.exists() {
        return Ok(None);
    }
    let p: Part = serde_json::from_slice(&std::fs::read(path)?)?;
    if now().saturating_sub(p.fetched_at) > max_age_days * 86400 {
        return Ok(None);
    }
    Ok(Some(p))
}
pub fn clear_cache() -> Result<usize> {
    let dir = cache_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let n = std::fs::read_dir(&dir)?.count();
    std::fs::remove_dir_all(dir)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes() {
        assert_eq!(normalize_part(" 123 "), "C123");
        assert_eq!(normalize_part("c42"), "C42");
    }
}

fn lookup_easyeda(id: &str) -> Result<Option<Part>> {
    let url = format!("https://easyeda.com/api/products/{id}/components?version=6.4.19.5");
    let response = client()?
        .get(&url)
        .send()
        .with_context(|| format!("parts capability unavailable: GET {url}"))?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "parts capability unavailable: GET {url} returned HTTP {status}"
    );
    let root: Value = response
        .json()
        .with_context(|| format!("invalid part response from {url}"))?;
    parse_easyeda(&root, id).with_context(|| format!("EasyEDA endpoint {url}"))
}

fn parse_easyeda(root: &Value, id: &str) -> Result<Option<Part>> {
    if root["success"] != true {
        if root["code"] == 404 {
            return Ok(None);
        }
        bail!(
            "parts capability unavailable: EasyEDA returned {}: {}",
            root["code"],
            root["message"]
        );
    }
    let result = root.get("result").context("missing result")?;
    let data = match &result["dataStr"] {
        Value::String(s) => serde_json::from_str(s).context("invalid component dataStr")?,
        value => value.clone(),
    };
    let params = &data["head"]["c_para"];
    let s = |key: &str| params[key].as_str().unwrap_or("").to_owned();
    let lcsc = &result["lcsc"];
    let returned_id = lcsc["number"].as_str().unwrap_or("");
    anyhow::ensure!(
        returned_id == id || s("Supplier Part") == id,
        "part identity does not match requested {id}"
    );
    let mpn = s("Manufacturer Part");
    anyhow::ensure!(!mpn.is_empty(), "missing manufacturer part number for {id}");
    Ok(Some(Part {
        source: "easyeda".into(),
        stock_source: "lcsc".into(),
        lcsc_part: id.into(),
        mfr_part: mpn,
        manufacturer: s("Manufacturer"),
        description: result["description"].as_str().unwrap_or("").into(),
        package: s("package"),
        stock: lcsc["stock"].as_i64().unwrap_or(-1),
        min_order: lcsc["min"].as_u64().unwrap_or(1),
        // EasyEDA stock/price is LCSC stock, not JLCPCB assembly availability.
        prices: Vec::new(),
        is_basic: s("JLCPCB Part Class") == "Basic Part",
        datasheet_url: result["datasheet"].as_str().unwrap_or("").into(),
        product_url: lcsc["url"].as_str().unwrap_or("").into(),
        fetched_at: now(),
    }))
}

#[cfg(test)]
mod easyeda_tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::json!({"success":true,"result":{
            "lcsc":{"number":"C70381","stock":10106,"min":1,"url":"https://lcsc.com/C70381"},
            "dataStr":{"head":{"c_para":{"Supplier Part":"C70381","Manufacturer Part":"CR1220-2","Manufacturer":"QIJEY","package":"BAT-SMD_CR1220-2","JLCPCB Part Class":"Extended Part"}}}
        }})
    }
    #[test]
    fn maps_object_and_string_component_data_without_inventing_assembly_stock() {
        let mut value = fixture();
        for _ in 0..2 {
            let part = parse_easyeda(&value, "C70381").unwrap().unwrap();
            assert_eq!(part.mfr_part, "CR1220-2");
            assert_eq!(part.manufacturer, "QIJEY");
            assert_eq!(part.stock_source, "lcsc");
            assert_eq!(part.stock, 10106);
            assert!(part.prices.is_empty());
            value["result"]["dataStr"] = value["result"]["dataStr"].to_string().into();
        }
    }
    #[test]
    fn missing_stock_is_unknown_and_mismatched_ids_fail() {
        let mut value = fixture();
        value["result"]["lcsc"]["stock"] = Value::Null;
        assert_eq!(parse_easyeda(&value, "C70381").unwrap().unwrap().stock, -1);
        assert!(parse_easyeda(&value, "C1").is_err());
        assert!(
            parse_easyeda(&serde_json::json!({"success":false,"code":404}), "C1")
                .unwrap()
                .is_none()
        );
        assert!(parse_easyeda(&serde_json::json!({"success":false,"code":500}), "C1").is_err());
    }

    #[test]
    fn parses_v2_jlc_fields() {
        let part = parse_part(&serde_json::json!({"componentCode":"C5213", "componentModelEn":"LM358P", "componentBrandEn":"Texas Instruments", "componentSpecificationEn":"DIP-8", "stockCount":100, "componentPrices":[{"startNumber":1,"productPrice":0.22}]})).unwrap();
        assert_eq!(part.package, "DIP-8");
        assert_eq!(part.prices.len(), 1);
        assert_eq!(part.stock_source, "jlcpcb");
    }

    #[test]
    fn endpoint_failure_includes_url_and_http_status() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/catalog", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(count > 0);
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let error = post(&url, serde_json::json!({})).unwrap_err().to_string();
        server.join().unwrap();
        assert!(error.contains(&url));
        assert!(error.contains("404"));
        assert!(error.contains("capability unavailable"));
    }

    #[test]
    #[ignore = "live provider smoke test; requires network"]
    fn live_catalog_lookup_search_and_fallback() {
        let part = lookup("C70381").unwrap().unwrap();
        assert_eq!(part.lcsc_part, "C70381");
        assert!(!part.mfr_part.is_empty());
        assert_eq!(part.source, "jlcpcb");
        let results = search("LM358", 2, false, false).unwrap();
        assert!(!results.parts.is_empty());
        assert!(results.parts.iter().all(|p| !p.lcsc_part.is_empty()));
        let fallback = lookup_easyeda("C70381").unwrap().unwrap();
        assert_eq!(fallback.stock_source, "lcsc");
        assert_eq!(fallback.mfr_part, "CR1220-2");
    }
}
