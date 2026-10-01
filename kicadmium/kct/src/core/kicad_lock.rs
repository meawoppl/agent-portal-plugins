//! Advisory presence checks for KiCad's `~<file>.lck` markers (port of
//! `kicad_tools.core.kicad_lock`).
//!
//! Ownership metadata never establishes process liveness; this neither
//! acquires a lock nor prevents another process from opening the file.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::utils::pyrepr::py_str_repr;

pub const LOCK_POLICY_ENV: &str = "KCT_KICAD_LOCK_POLICY";

const MAX_MARKER_CHARS: usize = 8192;

/// Observed marker presence with optional (unverified) ownership metadata.
/// Markers that cannot be inspected count as present; empty, unreadable, and
/// malformed markers have unknown ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiCadLockPresence {
    pub path: PathBuf,
    pub present: bool,
    pub username: Option<String>,
    pub hostname: Option<String>,
}

impl KiCadLockPresence {
    fn present(path: PathBuf) -> Self {
        Self {
            path,
            present: true,
            username: None,
            hostname: None,
        }
    }
}

/// Sibling marker path: `dir/~<name>.lck`.
pub fn lock_marker_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("~{name}.lck"))
}

/// Inspect only the exact sibling marker; never modify it or infer liveness.
pub fn probe_kicad_lock(path: impl AsRef<Path>) -> KiCadLockPresence {
    let marker = lock_marker_path(path.as_ref());
    let meta = match std::fs::symlink_metadata(&marker) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return KiCadLockPresence {
                path: marker,
                present: false,
                username: None,
                hostname: None,
            }
        }
        Err(_) => return KiCadLockPresence::present(marker),
    };
    if !meta.file_type().is_file() {
        return KiCadLockPresence::present(marker);
    }
    let Some(text) = read_marker(&marker) else {
        return KiCadLockPresence::present(marker);
    };
    if text.chars().count() > MAX_MARKER_CHARS {
        return KiCadLockPresence::present(marker);
    }
    let Ok(serde_json::Value::Object(data)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return KiCadLockPresence::present(marker);
    };
    let field = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
    KiCadLockPresence {
        username: field("username"),
        hostname: field("hostname"),
        path: marker,
        present: true,
    }
}

fn read_marker(marker: &Path) -> Option<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        // O_NONBLOCK: never hang on a FIFO swapped in after the lstat check
        // (symlinks and other non-regular markers were rejected before open).
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const O_NONBLOCK: i32 = 0o4000;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const O_NONBLOCK: i32 = 0x0004;
        options.custom_flags(O_NONBLOCK);
    }
    let file = options.open(marker).ok()?;
    if !file.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    // At most MAX_MARKER_CHARS + 1 code points of up to 4 bytes each.
    file.take(((MAX_MARKER_CHARS + 1) * 4) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Lock policy selected for a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPolicy {
    Warn,
    Error,
    Ignore,
}

impl LockPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "ignore" => Some(Self::Ignore),
            _ => None,
        }
    }
}

/// Failure of [`check_kicad_lock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiCadLockError {
    /// Invalid policy value (Python `ValueError`).
    InvalidPolicy(String),
    /// Policy `error` and a marker is present (Python `PermissionError`).
    Locked(String),
}

impl fmt::Display for KiCadLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(m) | Self::Locked(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for KiCadLockError {}

/// Apply `warn` (default), `error`, or `ignore` before a write. An explicit
/// `policy` overrides `KCT_KICAD_LOCK_POLICY`. `ignore` skips filesystem
/// inspection; `warn` prints only to stderr.
pub fn check_kicad_lock(
    path: impl AsRef<Path>,
    policy: Option<&str>,
) -> Result<Option<KiCadLockPresence>, KiCadLockError> {
    let path = path.as_ref();
    let env = std::env::var(LOCK_POLICY_ENV).ok();
    let selected = policy
        .map(str::to_string)
        .or(env)
        .unwrap_or_else(|| "warn".to_string());
    let parsed = LockPolicy::parse(&selected).ok_or_else(|| {
        KiCadLockError::InvalidPolicy(format!(
            "Invalid {LOCK_POLICY_ENV} value {}; expected warn, error or ignore",
            py_str_repr(&selected)
        ))
    })?;
    if parsed == LockPolicy::Ignore {
        return Ok(None);
    }
    let presence = probe_kicad_lock(path);
    if presence.present {
        let message = lock_message(path, &presence);
        if parsed == LockPolicy::Error {
            return Err(KiCadLockError::Locked(message));
        }
        eprintln!("Warning: {message}");
    }
    Ok(Some(presence))
}

fn lock_message(path: &Path, presence: &KiCadLockPresence) -> String {
    let owner: Vec<String> = [
        ("username", &presence.username),
        ("hostname", &presence.hostname),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.as_ref().map(|v| format!("{name}={}", py_str_repr(v))))
    .collect();
    let owner = if owner.is_empty() {
        "unknown ownership".to_string()
    } else {
        owner.join(", ")
    };
    format!(
        "{} may be open in KiCad: lock marker {} ({owner}). Marker presence does not prove a \
         live session. Set {LOCK_POLICY_ENV}=ignore to bypass this advisory check.",
        py_str_repr(&path.to_string_lossy()),
        py_str_repr(&presence.path.to_string_lossy()),
    )
}
