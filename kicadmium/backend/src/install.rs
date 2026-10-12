//! Publish a complete executable outside Cargo's mutable build directory.
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub(crate) fn activate(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("install destination needs a parent")?;
    fs::create_dir_all(parent)?;
    let mut input = fs::File::open(source).context("open built executable")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut input, &mut staged)?;
    staged
        .as_file()
        .set_permissions(input.metadata()?.permissions())?;
    staged.as_file().sync_all()?;
    staged
        .persist(destination)
        .context("atomically activate executable")?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub(crate) fn build_status() -> serde_json::Value {
    let root = std::env::var_os("AGENT_PORTAL_PLUGIN_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()?
                .parent()?
                .parent()
                .map(Path::to_path_buf)
        });
    let marker = root.map(|root| root.join(".install-lock/pid"));
    let pid = marker.as_ref().and_then(|p| fs::read_to_string(p).ok());
    serde_json::json!({"rebuild_in_progress":pid.is_some(),"installer_pid":pid.map(|p|p.trim().to_owned()),"marker":marker})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readers_always_see_a_complete_binary() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bin/kicadmium");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, vec![1; 65536]).unwrap();
        fs::write(&b, vec![2; 65536]).unwrap();
        activate(&a, &dest).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..200 {
                    let bytes = fs::read(&dest).unwrap();
                    assert_eq!(bytes.len(), 65536);
                    assert!(bytes.iter().all(|b| *b == bytes[0]));
                }
            });
            for _ in 0..20 {
                activate(&b, &dest).unwrap();
                activate(&a, &dest).unwrap();
            }
        });
        assert!(activate(&dir.path().join("missing"), &dest).is_err());
        assert_eq!(fs::read(&dest).unwrap(), vec![1; 65536]);
    }
}
