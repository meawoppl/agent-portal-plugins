use crate::lint::Report;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub board_id: String,
    pub key: String,
    pub evidence: String,
    pub action: String,
    pub reason: String,
    pub reviewer: String,
    pub reviewed_at: u64,
    pub expires_at: Option<u64>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub schema: u32,
    pub decisions: Vec<Decision>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Audit {
    pub key: String,
    pub status: String,
    pub reason: String,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn load(path: &Path) -> Result<Ledger> {
    if !path.exists() {
        return Ok(Ledger {
            schema: 1,
            decisions: vec![],
        });
    }
    let l: Ledger = serde_json::from_str(&fs::read_to_string(path)?)?;
    if l.schema != 1 {
        bail!("unsupported review schema")
    }
    let mut keys = std::collections::BTreeSet::new();
    for d in &l.decisions {
        if !["ignore", "flag"].contains(&d.action.as_str())
            || d.reason.trim().is_empty()
            || d.reviewer.trim().is_empty()
            || !keys.insert((&d.board_id, &d.key))
        {
            bail!("invalid/duplicate review decision")
        }
    }
    Ok(l)
}
pub fn apply(r: &mut Report, l: &Ledger, at: u64) {
    r.review_audit.clear();
    for f in &mut r.findings {
        f.state = "open".into();
        f.review = None;
    }
    for d in &l.decisions {
        if d.board_id != r.board_id {
            continue;
        }
        let status = if let Some(f) = r.findings.iter_mut().find(|f| f.key == d.key) {
            if d.expires_at.is_some_and(|t| t <= at) {
                "expired"
            } else if !f.stable_identity || f.evidence != d.evidence {
                "changed"
            } else {
                f.state = if d.action == "ignore" {
                    "ignored"
                } else {
                    "flagged"
                }
                .into();
                f.review = Some(d.clone());
                "applied"
            }
        } else {
            "not_observed"
        };
        r.review_audit.push(Audit {
            key: d.key.clone(),
            status: status.into(),
            reason: d.reason.clone(),
        });
    }
}
/// Refuse to write `output` over a KiCad design file or over any of `inputs`
/// (compared by canonical path, so `./a` and `a` alias).
pub fn protect_output(output: Option<&Path>, inputs: &[&Path]) -> Result<()> {
    let Some(out) = output else {
        return Ok(());
    };
    if out
        .extension()
        .is_some_and(|e| e == "kicad_pcb" || e == "kicad_sch" || e == "kicad_pro")
    {
        bail!("refusing to overwrite a KiCad design with JSON")
    }
    let identity = |p: &Path| -> Result<std::path::PathBuf> {
        if p.exists() {
            Ok(p.canonicalize()?)
        } else if p.is_absolute() {
            Ok(p.to_owned())
        } else {
            Ok(std::env::current_dir()?.join(p))
        }
    };
    let target = identity(out)?;
    for input in inputs {
        if target == identity(input)? {
            bail!("output aliases an input: {}", input.display())
        }
    }
    Ok(())
}
pub fn atomic_json<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(data)?)
}
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .context("missing filename")?
            .to_string_lossy(),
        std::process::id()
    ));
    let mut owned = false;
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        owned = true;
        f.write_all(data)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() && owned {
        let _ = fs::remove_file(temp);
    }
    result
}
/// Exclusive advisory lock held by creating `lock_path`; removed on drop.
/// Stale locks are reported, never stolen.
pub struct LockGuard(std::path::PathBuf);
impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn lock(lock_path: &Path) -> Result<LockGuard> {
    let parent = lock_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
        .with_context(|| {
            format!(
                "{} is locked; check running writers before removing a stale lock",
                lock_path.display()
            )
        })?;
    let guard = LockGuard(lock_path.to_owned());
    writeln!(file, "pid={}", std::process::id())?;
    Ok(guard)
}
/// Lock protects concurrent read-modify-write; stale locks are explicit, never stolen.
pub fn update(
    path: &Path,
    report: &Report,
    key: &str,
    action: &str,
    reason: &str,
    reviewer: &str,
    expires_at: Option<u64>,
) -> Result<()> {
    if report.schema != 1 {
        bail!("unsupported report schema")
    }
    let found = report
        .findings
        .iter()
        .find(|f| f.key == key)
        .context("finding key absent from report (use full key)")?;
    if !["ignore", "flag", "clear"].contains(&action) {
        bail!("expected ignore, flag or clear")
    }
    if action != "clear" && (reason.trim().is_empty() || reviewer.trim().is_empty()) {
        bail!("reason and reviewer are required")
    }
    if action == "ignore" && !found.stable_identity {
        bail!(
            "cannot persistently ignore an object without stable UUID; add UUID or encode intent in configuration"
        )
    }
    if expires_at.is_some_and(|t| t <= now()) {
        bail!("expiry must be in the future")
    }
    let _guard = lock(&path.with_extension("lock"))
        .context("review ledger locked; check running writers before removing a stale .lock")?;
    let mut ledger = load(path)?;
    ledger
        .decisions
        .retain(|d| !(d.board_id == report.board_id && d.key == key));
    if action != "clear" {
        ledger.decisions.push(Decision {
            board_id: report.board_id.clone(),
            key: key.into(),
            evidence: found.evidence.clone(),
            action: action.into(),
            reason: reason.into(),
            reviewer: reviewer.into(),
            reviewed_at: now(),
            expires_at,
        });
    }
    ledger
        .decisions
        .sort_by(|a, b| (&a.board_id, &a.key).cmp(&(&b.board_id, &b.key)));
    atomic_json(path, &ledger)
}
