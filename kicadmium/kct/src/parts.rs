//! LCSC/JLCPCB component catalog client and persistent cache.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DETAIL: &str =
    "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentDetail";
const SEARCH: &str =
    "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    pub quantity: u64,
    pub unit_price: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
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
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub parts: Vec<Part>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
    pub source: &'static str,
}
fn client() -> Result<reqwest::blocking::Client> {
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
        .context("JLCPCB parts API request failed")?;
    let status = response.status();
    if !status.is_success() {
        bail!("JLCPCB parts API returned HTTP {status}")
    }
    response.json().context("invalid JLCPCB parts API response")
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
    let root = post(DETAIL, serde_json::json!({"componentCode":id}))?;
    if root.get("code").and_then(Value::as_i64) != Some(200) {
        bail!(
            "JLCPCB API business error: {}",
            root.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )
    }
    match root.get("data") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(parse_part(v)?)),
    }
}
pub fn search(query: &str, page_size: u64, in_stock: bool, basic: bool) -> Result<SearchResult> {
    let mut payload =
        serde_json::json!({"keyword":query,"pageSize":page_size.min(100),"currentPage":1});
    if in_stock {
        payload["stockCountMin"] = 1.into()
    }
    if basic {
        payload["componentLibraryType"] = "base".into()
    }
    let root = post(SEARCH, payload)?;
    if root.get("code").and_then(Value::as_i64) != Some(200) {
        bail!("JLCPCB API business error")
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
        source: "live",
    })
}
fn parse_part(v: &Value) -> Result<Part> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let prices = v
        .get("prices")
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
    Ok(Part {
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
                s("package")
            } else {
                x
            }
        },
        stock: v.get("stockCount").and_then(Value::as_i64).unwrap_or(0),
        min_order: v.get("minOrder").and_then(Value::as_u64).unwrap_or(1),
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
