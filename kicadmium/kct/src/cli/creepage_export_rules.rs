//! Export voltage-domain clearance rules into a KiCad `.kicad_dru` file.

use std::ffi::OsString;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};

const BEGIN: &str = "# BEGIN KICADMIUM CREEPAGE RULES";
const END: &str = "# END KICADMIUM CREEPAGE RULES";

#[derive(Parser)]
#[command(about = "Generate KiCad pairwise voltage-domain clearance rules")]
struct Args {
    project: PathBuf,
    #[arg(long)]
    pcb: Option<PathBuf>,
    #[arg(long = "voltage-map")]
    voltage_map: Option<PathBuf>,
    #[arg(long, default_value = "iec60664")]
    standard: String,
    #[arg(long = "pollution-degree", default_value_t = 2)]
    pollution_degree: u8,
    #[arg(long = "material-group", default_value = "IIIa")]
    material_group: String,
    #[arg(long = "hv-threshold", default_value_t = 30.0)]
    hv_threshold: f64,
    #[arg(long = "dru-floor", default_value_t = 0.2)]
    dru_floor: f64,
    /// Explicit destination `.kicad_dru`; required unless `--dry-run`.
    #[arg(short, long, value_name = "DRU")]
    output: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Serialize)]
struct Rule {
    name: String,
    condition: String,
    min_mm: f64,
}

fn load_map(path: &PathBuf) -> Result<BTreeMap<String, f64>> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let object =
        value.get("nets").unwrap_or(&value).as_object().context(
            "voltage map must be an object mapping net names to numbers or voltage objects",
        )?;
    object
        .iter()
        .map(|(name, v)| {
            let voltage = v
                .as_f64()
                .or_else(|| v.get("voltage").and_then(serde_json::Value::as_f64))
                .or_else(|| {
                    Some(
                        v.get("lo")?
                            .as_f64()?
                            .abs()
                            .max(v.get("hi")?.as_f64()?.abs()),
                    )
                })
                .with_context(|| format!("invalid voltage for {name}"))?;
            Ok((name.clone(), voltage))
        })
        .collect()
}

fn clearance(delta_v: f64, floor: f64, pd: u8, group: &str) -> Result<f64> {
    if !(1..=3).contains(&pd) {
        bail!("pollution degree must be 1, 2, or 3");
    }
    let base: f64 = match delta_v {
        v if v <= 32.0 => 0.53,
        v if v <= 50.0 => 1.2,
        v if v <= 100.0 => 1.4,
        v if v <= 160.0 => 1.6,
        v if v <= 250.0 => 2.5,
        v if v <= 400.0 => 4.0,
        v if v <= 630.0 => 6.3,
        v if v <= 1000.0 => 10.0,
        _ => bail!("voltage delta {delta_v} V exceeds supported IEC table"),
    };
    let material: f64 = match group.to_ascii_lowercase().as_str() {
        "i" => 0.65,
        "ii" => 0.8,
        "iiia" | "iii" | "3a" => 1.0,
        "iiib" | "3b" => 1.25,
        _ => bail!("unknown material group {group:?}"),
    };
    Ok(floor.max(base * [0.0, 0.5, 1.0, 1.6][pd as usize] * material))
}

fn slug(voltage: f64) -> String {
    format!("V{}", voltage.abs().round() as i64).replace('-', "N")
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("creepage-export-rules", args);
    if args.project.extension().and_then(|s| s.to_str()) != Some("kicad_pro") {
        bail!(
            "expected a .kicad_pro project file: {}",
            args.project.display()
        );
    }
    if !args.project.exists() {
        bail!("project file not found: {}", args.project.display());
    }
    let pcb = args
        .pcb
        .clone()
        .unwrap_or_else(|| args.project.with_extension("kicad_pcb"));
    if !args.dry_run && args.output.is_none() {
        bail!("creepage-export-rules writes design rules; pass --output (or use --dry-run)")
    }
    if args
        .output
        .as_ref()
        .is_some_and(|path| path.extension().and_then(|s| s.to_str()) != Some("kicad_dru"))
    {
        bail!("--output must be a .kicad_dru file")
    }
    let dru = args
        .output
        .clone()
        .unwrap_or_else(|| args.project.with_extension("kicad_dru"));
    let Some(map_path) = &args.voltage_map else {
        let document = serde_json::json!({"command":"creepage-export-rules","project":args.project,"pcb":pcb,"dru":dru,"voltage_map":null,"rules":[],"written":false,"skipped_reason":"no-voltage-map","success":true});
        if args.format == "json" {
            println!("{}", serde_json::to_string_pretty(&document)?);
        } else {
            println!("No --voltage-map supplied: nothing to export. No files written.");
        }
        return Ok(0);
    };
    if !pcb.exists() {
        bail!("board file not found: {}", pcb.display());
    }
    if !matches!(
        args.standard.to_ascii_lowercase().as_str(),
        "iec60664" | "iec62368"
    ) {
        bail!("unsupported standard {:?}", args.standard);
    }
    let map = load_map(map_path)?;
    let mut domains: BTreeMap<String, f64> = BTreeMap::new();
    for voltage in map.values().copied() {
        domains.entry(slug(voltage)).or_insert(voltage);
    }
    let values: Vec<_> = domains.iter().collect();
    let mut rules = Vec::new();
    for (i, (a, av)) in values.iter().enumerate() {
        for (b, bv) in values.iter().skip(i + 1) {
            let min = clearance(
                (*av - *bv).abs(),
                args.dru_floor,
                args.pollution_degree,
                &args.material_group,
            )?;
            if (*av - *bv).abs() >= args.hv_threshold {
                rules.push(Rule{name:format!("kicadmium_{}_to_{}",a,b),condition:format!("A.NetClass == '{}' && B.NetClass == '{}' || A.NetClass == '{}' && B.NetClass == '{}'",a,b,b,a),min_mm:min});
            }
        }
    }
    let body = rules
        .iter()
        .map(|r| {
            format!(
                "(rule \"{}\"\n  (condition \"{}\")\n  (constraint clearance (min {:.4}mm)))",
                r.name, r.condition, r.min_mm
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let block = format!(
        "{BEGIN}\n# Engineering aid only; verify against the governing standard.\n{body}\n{END}"
    );
    let mut written = false;
    if !args.dry_run {
        let previous = std::fs::read_to_string(&dru).unwrap_or_default();
        let merged = if let (Some(start), Some(end)) = (previous.find(BEGIN), previous.find(END)) {
            format!(
                "{}{}{}",
                &previous[..start],
                block,
                &previous[end + END.len()..]
            )
        } else if previous.trim().is_empty() {
            format!("(version 1)\n\n{block}\n")
        } else {
            format!("{}\n\n{block}\n", previous.trim_end())
        };
        std::fs::write(&dru, merged).with_context(|| format!("write {}", dru.display()))?;
        written = true;
    }
    let document = serde_json::json!({"command":"creepage-export-rules","project":args.project,"pcb":pcb,"dru":dru,"voltage_map":map_path,"standard":args.standard,"pollution_degree":args.pollution_degree,"material_group":args.material_group,"domains":domains,"nets_assigned":map.len(),"rules":rules,"dry_run":args.dry_run,"written":written,"success":true,"dru_block":if args.dry_run{Some(block.as_str())}else{None}});
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else if args.dry_run {
        println!("{block}");
    } else {
        println!(
            "Wrote {} pairwise rule(s) -> {}",
            rules.len(),
            dru.display()
        );
    }
    Ok(0)
}
