use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_yaml::{Mapping, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
struct InitArgs {
    project: PathBuf,
    #[arg(short = 'm', long)]
    mfr: String,
    #[arg(short = 'l', long, default_value_t = 2)]
    layers: u8,
    #[arg(short = 'c', long, default_value_t = 1.0)]
    copper: f64,
    #[arg(short = 't', long)]
    design_type: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
pub fn init(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<InitArgs>("init", args);
    let profile = crate::manufacturers::profile(&a.mfr)?;
    let (dir, project) = if a.project.extension().and_then(|x| x.to_str()) == Some("kicad_pro") {
        (
            a.project.parent().unwrap_or(Path::new(".")).to_owned(),
            a.project,
        )
    } else if a.project == Path::new(".") {
        let dir = std::env::current_dir()?;
        let name = dir
            .file_name()
            .context("current directory has no name")?
            .to_string_lossy();
        (dir.clone(), dir.join(format!("{name}.kicad_pro")))
    } else {
        let dir = a.project.clone();
        let name = dir
            .file_name()
            .context("project path has no name")?
            .to_string_lossy();
        (dir.clone(), dir.join(format!("{name}.kicad_pro")))
    };
    let dru = dir.join(format!(
        "{}.kicad_dru",
        project.file_stem().unwrap().to_string_lossy()
    ));
    let config = dir.join("kicad-tools.toml");
    let planned = vec![project.clone(), dru.clone(), config.clone()];
    if !a.dry_run {
        std::fs::create_dir_all(&dir)?;
        if !project.exists() {
            crate::fsutil::atomic_write(&project,b"{\n  \"board\": {},\n  \"boards\": [],\n  \"cvpcb\": {},\n  \"erc\": {},\n  \"libraries\": {},\n  \"meta\": {\"filename\": \"\", \"version\": 1},\n  \"net_settings\": {},\n  \"pcbnew\": {},\n  \"schematic\": {},\n  \"text_variables\": {}\n}\n")?
        }
        let rules = crate::manufacturers::dru_preset(&a.mfr, a.layers, a.copper)?;
        crate::fsutil::atomic_write(&dru, rules.as_bytes())?;
        let cfg = format!(
            "[defaults]\nmanufacturer = {:?}\nlayers = {}\ncopper_oz = {}\n{}",
            profile.id,
            a.layers,
            a.copper,
            a.design_type
                .as_ref()
                .map_or(String::new(), |v| format!("design_type = {v:?}\n"))
        );
        crate::fsutil::atomic_write(&config, cfg.as_bytes())?
    }
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"manufacturer":profile.id,"dry_run":a.dry_run,"files":planned})
            )?
        )
    } else {
        println!(
            "{} project for {}",
            if a.dry_run {
                "Would initialize"
            } else {
                "Initialized"
            },
            profile.name
        );
        for p in planned {
            println!("  {}", p.display())
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct SpecArgs {
    #[command(subcommand)]
    command: SpecCommand,
}
#[derive(Subcommand)]
enum SpecCommand {
    Init {
        name: String,
        #[arg(short = 't', long, default_value = "minimal")]
        template: String,
        #[arg(short, long, default_value = "project.kct")]
        output: PathBuf,
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Validate {
        file: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Status {
        file: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Decide {
        file: PathBuf,
        #[arg(long)]
        topic: String,
        #[arg(long)]
        choice: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        alternatives: Option<String>,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Check {
        file: PathBuf,
        item: String,
        #[arg(long, default_value = "text")]
        format: String,
    },
}
pub fn spec(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<SpecArgs>("spec", args).command {
        SpecCommand::Init {
            name,
            template,
            output,
            force,
            format,
        } => {
            if output.exists() && !force {
                bail!("{} exists (use --force)", output.display())
            }
            let value = template_spec(&name, &template)?;
            save_yaml(&output, &value)?;
            emit(
                &format,
                serde_json::json!({"created":output,"template":template}),
                format!("Created {}", output.display()),
            )?
        }
        SpecCommand::Validate { file, format } => {
            let v = load_yaml(&file)?;
            let errors = validate_spec(&v);
            let ok = errors.is_empty();
            emit(
                &format,
                serde_json::json!({"valid":ok,"errors":errors}),
                if ok {
                    "Valid specification".into()
                } else {
                    format!("Invalid: {}", errors.join("; "))
                },
            )?;
            return Ok(if ok { 0 } else { 1 });
        }
        SpecCommand::Status { file, format } => {
            let v = load_yaml(&file)?;
            let project = v
                .get("project")
                .and_then(|x| x.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            let progress = v.get("progress").cloned().unwrap_or(Value::Null);
            emit(
                &format,
                serde_json::json!({"project":project,"progress":progress}),
                format!(
                    "Project: {project}\nProgress:\n{}",
                    serde_yaml::to_string(&progress)?
                ),
            )?
        }
        SpecCommand::Decide {
            file,
            topic,
            choice,
            rationale,
            alternatives,
            format,
        } => {
            let mut v = load_yaml(&file)?;
            let decision = serde_yaml::to_value(
                serde_json::json!({"topic":topic,"choice":choice,"rationale":rationale,"alternatives":alternatives.map(|s|s.split(',').map(|v|v.trim().to_owned()).collect::<Vec<_>>()),"timestamp":now()}),
            )?;
            mapping_mut(&mut v)?
                .entry(Value::String("decisions".into()))
                .or_insert_with(|| Value::Sequence(vec![]))
                .as_sequence_mut()
                .context("decisions must be a list")?
                .push(decision);
            save_yaml(&file, &v)?;
            emit(
                &format,
                serde_json::json!({"updated":file,"topic":topic}),
                format!("Recorded decision: {topic}"),
            )?
        }
        SpecCommand::Check { file, item, format } => {
            let mut v = load_yaml(&file)?;
            let progress = mapping_mut(&mut v)?
                .entry(Value::String("progress".into()))
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            mapping_mut(progress)?.insert(Value::String(item.clone()), Value::Bool(true));
            save_yaml(&file, &v)?;
            emit(
                &format,
                serde_json::json!({"updated":file,"item":item}),
                format!("Completed: {item}"),
            )?
        }
    }
    Ok(0)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn template_spec(name: &str, template: &str) -> Result<Value> {
    if !["minimal", "power_supply", "sensor_board", "mcu_breakout"].contains(&template) {
        bail!("unknown template {template}")
    }
    let mut v:Value=serde_yaml::from_str(&format!("version: 1\nproject:\n  name: {name:?}\n  revision: A\nintent:\n  summary: {name:?}\nrequirements: {{}}\ndecisions: []\nprogress:\n  concept: pending\n  schematic: pending\n  layout: pending\n"))?;
    if template != "minimal" {
        mapping_mut(&mut v)?.insert(
            Value::String("template".into()),
            Value::String(template.into()),
        );
    }
    Ok(v)
}
fn load_yaml(path: &Path) -> Result<Value> {
    serde_yaml::from_str(
        &std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    )
    .context("invalid .kct YAML")
}
fn save_yaml(path: &Path, v: &Value) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?
    }
    crate::fsutil::atomic_write(path, serde_yaml::to_string(v)?.as_bytes())
}
fn mapping_mut(v: &mut Value) -> Result<&mut Mapping> {
    v.as_mapping_mut().context("expected mapping")
}
fn validate_spec(v: &Value) -> Vec<String> {
    let mut e = vec![];
    if !v.is_mapping() {
        e.push("root must be a mapping".into());
        return e;
    }
    if v.get("project")
        .and_then(|x| x.get("name"))
        .and_then(Value::as_str)
        .is_none()
    {
        e.push("project.name is required".into())
    }
    if v.get("intent")
        .and_then(|x| x.get("summary"))
        .and_then(Value::as_str)
        .is_none()
    {
        e.push("intent.summary is required".into())
    }
    e
}
fn emit(format: &str, json: serde_json::Value, text: String) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&json)?)
    } else {
        println!("{text}")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_validate() {
        for t in ["minimal", "power_supply", "sensor_board", "mcu_breakout"] {
            assert!(validate_spec(&template_spec("x", t).unwrap()).is_empty())
        }
    }
}
