//! Workbench processes live in the user manager, outside Portal's cgroup.
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
#[derive(clap::Subcommand, Debug)]
pub(crate) enum Action {
    /// Idempotently start a supervised workbench and return its local URL.
    Ensure {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long, default_value_t = 8765)]
        port: u16,
        #[arg(long, default_value = "")]
        session: String,
        /// Keep the Portal launcher attached while the supervisor owns the server.
        #[arg(long)]
        wait: bool,
    },
}
fn unit(root: &Path, port: u16) -> String {
    let h = Sha256::digest(format!("{}:{port}", root.display()).as_bytes());
    format!("kicadmium-surface-{h:x}.service")
}
fn run(cmd: &mut Command) -> Result<()> {
    let output = cmd
        .output()
        .context("user systemd or launchd manager is required for supervised surfaces")?;
    if !output.status.success() {
        bail!(
            "surface supervisor: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}
fn registry(exe: &Path) -> Result<PathBuf> {
    Ok(exe
        .parent()
        .context("executable has no parent")?
        .join(".surface-units"))
}
pub(crate) fn restart_installed(exe: &Path) -> Result<()> {
    let registry = registry(exe)?;
    if !registry.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(registry)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with("kicadmium-surface-") && name.ends_with(".service") {
            if cfg!(target_os = "macos") {
                run(Command::new("launchctl").args([
                    "kickstart",
                    "-k",
                    &format!("{}/{}", launch_domain(), name),
                ]))?;
            } else {
                run(Command::new("systemctl").args(["--user", "try-restart", &name]))?;
            }
        }
    }
    Ok(())
}
pub(crate) fn execute(action: Action) -> Result<()> {
    match action {
        Action::Ensure {
            cwd,
            port,
            session,
            wait,
        } => {
            if port == 0 {
                bail!("surface needs a stable nonzero port")
            }
            let cwd = cwd.canonicalize()?;
            let exe = std::env::current_exe()?.canonicalize()?;
            if exe
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                != Some("bin")
            {
                bail!("run surface ensure with the installed bin/kicadmium executable so updates preserve its path")
            }
            let name = unit(&cwd, port);
            let active = if cfg!(target_os = "macos") {
                Command::new("launchctl")
                    .args(["print", &format!("{}/{}", launch_domain(), name)])
                    .output()?
                    .status
                    .success()
            } else {
                Command::new("systemctl")
                    .args(["--user", "is-active", "--quiet", &name])
                    .status()?
                    .success()
            };
            if !active {
                if std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                    std::time::Duration::from_millis(100),
                )
                .is_ok()
                {
                    bail!("port {port} is already occupied by a process outside this surface unit")
                }
                if cfg!(target_os = "macos") {
                    let dir = registry(&exe)?;
                    std::fs::create_dir_all(&dir)?;
                    let path = dir.join(&name);
                    std::fs::write(&path, launch_plist(&name, &exe, &cwd, port, &session))?;
                    run(Command::new("launchctl")
                        .arg("bootstrap")
                        .arg(launch_domain())
                        .arg(path))?;
                } else {
                    // Clear a failed transient unit before reusing its deterministic name.
                    let _ = Command::new("systemctl")
                        .args(["--user", "reset-failed", &name])
                        .output();
                    run(Command::new("systemd-run")
                        .args([
                            "--user",
                            "--collect",
                            "--unit",
                            &name,
                            "--property=Restart=always",
                            "--property=RestartSec=2",
                            "--property=KillMode=mixed",
                        ])
                        .arg(&exe)
                        .arg("serve")
                        .arg("--cwd")
                        .arg(&cwd)
                        .arg("--port")
                        .arg(port.to_string())
                        .arg("--session")
                        .arg(session))?;
                }
            }
            let dir = registry(&exe)?;
            std::fs::create_dir_all(&dir)?;
            if !cfg!(target_os = "macos") {
                std::fs::write(dir.join(&name), cwd.to_string_lossy().as_bytes())?;
            }
            // Do not return a URL before the listener exists.
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            for _ in 0..100 {
                if healthy(address) {
                    println!("http://127.0.0.1:{port}");
                    if wait {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(30));
                        }
                    }
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            bail!("surface did not become ready; inspect journalctl --user -u {name}")
        }
    }
}
fn launch_domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}
fn healthy(address: std::net::SocketAddr) -> bool {
    use std::io::{Read, Write};
    let request = || -> Result<bool> {
        let mut stream =
            std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(100))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
        stream
            .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut response = String::new();
        stream.take(65536).read_to_string(&mut response)?;
        let Some((header, body)) = response.split_once("\r\n\r\n") else {
            return Ok(false);
        };
        let value: serde_json::Value = serde_json::from_str(body)?;
        Ok(header.contains(" 200 ") && value["plugin"] == "kicadmium" && value["ok"] == true)
    };
    request().unwrap_or(false)
}
fn launch_plist(name: &str, exe: &Path, cwd: &Path, port: u16, session: &str) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let args = [
        exe.to_string_lossy().into_owned(),
        "serve".into(),
        "--cwd".into(),
        cwd.to_string_lossy().into_owned(),
        "--port".into(),
        port.to_string(),
        "--session".into(),
        session.into(),
    ];
    let args = args
        .iter()
        .map(|a| format!("<string>{}</string>", escape(a)))
        .collect::<String>();
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{args}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>2</integer></dict></plist>",escape(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_stable_and_separates_workspaces_and_ports() {
        assert_eq!(unit(Path::new("/a"), 80), unit(Path::new("/a"), 80));
        assert_ne!(unit(Path::new("/a"), 80), unit(Path::new("/a"), 81));
        assert_ne!(unit(Path::new("/a"), 80), unit(Path::new("/b"), 80));
        assert!(!unit(Path::new("/with spaces/%"), 80).contains('%'));
    }
}
