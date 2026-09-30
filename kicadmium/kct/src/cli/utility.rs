//! Configuration, cleanup, and native environment diagnostics.
use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn project_config() -> PathBuf {
    PathBuf::from("kicad-tools.toml")
}
fn user_config() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("kicad-tools/config.toml"))
}
fn template() -> &'static str {
    "[defaults]\nformat = \"text\"\nunits = \"mm\"\nmanufacturer = \"jlcpcb\"\n\n[export]\noutput_dir = \"output\"\ninclude_dnp = false\n\n[parts]\ncache_ttl_days = 7\n"
}

#[derive(Parser)]
struct ConfigArgs {
    #[arg(long)]
    show: bool,
    #[arg(long)]
    init: bool,
    #[arg(long)]
    paths: bool,
    #[arg(long)]
    user: bool,
    #[arg(long, default_value = "text")]
    format: String,
    action: Option<String>,
    key: Option<String>,
    value: Option<String>,
}
pub fn config(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<ConfigArgs>("config", args);
    let project = project_config();
    let user = user_config();
    if a.paths {
        let data = serde_json::json!({"project": project, "user": user});
        if a.format == "json" {
            println!("{}", serde_json::to_string_pretty(&data)?);
        } else {
            println!(
                "project: {}\nuser: {}",
                project.display(),
                user.as_ref()
                    .map_or_else(|| "unavailable".into(), |p| p.display().to_string())
            );
        }
        return Ok(0);
    }
    if a.init {
        let path = if a.user {
            user.context("no user config directory")?
        } else {
            project
        };
        if path.exists() {
            bail!("{} already exists", path.display())
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fsutil::atomic_write(&path, template().as_bytes())?;
        println!("Created {}", path.display());
        return Ok(0);
    }
    let path = if project.exists() {
        project
    } else if user.as_ref().is_some_and(|p| p.exists()) {
        user.unwrap()
    } else {
        project
    };
    let mut table: toml::Table = if path.exists() {
        toml::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        toml::from_str(template())?
    };
    match a.action.as_deref() {
        Some("get") => {
            let key = a.key.context("config get requires KEY")?;
            let v = get_path(&table, &key).context("configuration key not found")?;
            if a.format == "json" {
                println!("{}", serde_json::to_string_pretty(v)?)
            } else {
                println!("{v}")
            }
        }
        Some("set") => {
            let key = a.key.context("config set requires KEY")?;
            let raw = a.value.context("config set requires VALUE")?;
            set_path(&mut table, &key, parse_value(&raw))?;
            crate::fsutil::atomic_write(&path, toml::to_string_pretty(&table)?.as_bytes())?;
            println!("Set {key} in {}", path.display());
        }
        Some(other) => bail!("unknown config action {other}"),
        None => {
            if a.format == "json" {
                println!("{}", serde_json::to_string_pretty(&table)?)
            } else {
                print!("{}", toml::to_string_pretty(&table)?)
            }
        }
    }
    Ok(0)
}
fn get_path<'a>(t: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut cur = t.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?
    }
    Some(cur)
}
fn set_path(t: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let mut parts = key.split('.').peekable();
    let mut cur = t;
    while let Some(p) = parts.next() {
        if parts.peek().is_none() {
            cur.insert(p.into(), value);
            return Ok(());
        }
        cur = cur
            .entry(p)
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .context("key prefix is not a table")?
    }
    bail!("empty key")
}
fn parse_value(s: &str) -> toml::Value {
    if let Ok(v) = s.parse::<bool>() {
        v.into()
    } else if let Ok(v) = s.parse::<i64>() {
        v.into()
    } else if let Ok(v) = s.parse::<f64>() {
        v.into()
    } else {
        s.into()
    }
}

#[derive(Parser)]
struct CleanArgs {
    project: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    deep: bool,
    #[arg(short = 'f', long)]
    force: bool,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(short, long)]
    verbose: bool,
}
#[derive(Serialize)]
struct CleanReport {
    project: PathBuf,
    dry_run: bool,
    files: Vec<PathBuf>,
    bytes: u64,
}
pub fn clean(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<CleanArgs>("clean", args);
    if a.project.extension().and_then(|x| x.to_str()) != Some("kicad_pro") {
        bail!("project must be a .kicad_pro file")
    }
    let root = a.project.parent().unwrap_or(Path::new("."));
    let stem = a
        .project
        .file_stem()
        .and_then(|x| x.to_str())
        .context("invalid project name")?;
    let mut files = vec![];
    for e in std::fs::read_dir(root)? {
        let p = e?.path();
        let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
        let backup = n.starts_with(&format!("{stem}-backups"))
            || n.ends_with('~')
            || n.ends_with(".bak")
            || n.ends_with(".lck");
        let generated = a.deep
            && (n == "output"
                || n == "gerbers"
                || n == "build"
                || n.ends_with("-bom.csv")
                || n.ends_with("-pos.csv"));
        if backup || generated {
            files.push(p)
        }
    }
    let bytes = files
        .iter()
        .map(|p| {
            if p.is_file() {
                p.metadata().map_or(0, |m| m.len())
            } else {
                0
            }
        })
        .sum();
    let dry = !a.force || a.dry_run;
    if !dry {
        for p in &files {
            if p.is_dir() {
                std::fs::remove_dir_all(p)?
            } else {
                std::fs::remove_file(p)?
            }
        }
    }
    let report = CleanReport {
        project: a.project,
        dry_run: dry,
        files,
        bytes,
    };
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?)
    } else {
        println!(
            "{} {} item(s), {} bytes",
            if dry { "Would remove" } else { "Removed" },
            report.files.len(),
            report.bytes
        );
        if a.verbose {
            for p in &report.files {
                println!("  {}", p.display())
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct DoctorArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    strict: bool,
}
#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
    remedy: Option<&'static str>,
}
pub fn doctor(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<DoctorArgs>("doctor", args);
    let mut checks = vec![];
    let cli = find_executable("kicad-cli");
    checks.push(match cli {
        Some(path) => {
            let out = Command::new(&path).arg("version").output();
            match out {
                Ok(o) if o.status.success() => Check {
                    name: "kicad-cli",
                    status: "ok",
                    detail: String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                    remedy: None,
                },
                Ok(o) => Check {
                    name: "kicad-cli",
                    status: "fail",
                    detail: String::from_utf8_lossy(&o.stderr).trim().to_owned(),
                    remedy: Some("install KiCad 8 or newer and put kicad-cli on PATH"),
                },
                Err(e) => Check {
                    name: "kicad-cli",
                    status: "fail",
                    detail: e.to_string(),
                    remedy: Some("install KiCad 8 or newer and put kicad-cli on PATH"),
                },
            }
        }
        None => Check {
            name: "kicad-cli",
            status: "fail",
            detail: "not found".into(),
            remedy: Some("install KiCad 8 or newer and put kicad-cli on PATH"),
        },
    });
    checks.push(Check {
        name: "project-root",
        status: if a.root.exists() { "ok" } else { "fail" },
        detail: a.root.display().to_string(),
        remedy: if a.root.exists() {
            None
        } else {
            Some("pass --root with an existing project directory")
        },
    });
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&checks)?)
    } else {
        for c in &checks {
            println!("{:<5} {:<14} {}", c.status, c.name, c.detail);
            if let Some(r) = c.remedy {
                println!("      remedy: {r}")
            }
        }
    }
    Ok(if a.strict && checks.iter().any(|c| c.status == "fail") {
        1
    } else {
        0
    })
}
fn find_executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|p| Path::new(p).join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_config() {
        let mut t = toml::Table::new();
        set_path(&mut t, "a.b", 3.into()).unwrap();
        assert_eq!(get_path(&t, "a.b").and_then(|x| x.as_integer()), Some(3));
    }
}
