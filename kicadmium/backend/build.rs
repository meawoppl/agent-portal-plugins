//! Builds the Yew frontend with trunk when `frontend/dist` is missing or older
//! than its inputs, so `cargo build -p backend` alone yields a working binary.
//! Trunk gets its own target dir: the outer cargo holds the workspace one.
//! Set `KICADMIUM_SKIP_FRONTEND_BUILD=1` to embed whatever dist exists.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const INPUTS: &[&str] = &[
    "src",
    "static",
    "index.html",
    "style.css",
    "Trunk.toml",
    "Cargo.toml",
];

fn newest(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    std::fs::read_dir(path)
        .ok()?
        .filter_map(|entry| newest(&entry.ok()?.path()))
        .max()
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap();
    let frontend = root.join("frontend");
    let dist_index = frontend.join("dist/index.html");

    println!("cargo:rerun-if-env-changed=KICADMIUM_SKIP_FRONTEND_BUILD");
    for input in INPUTS.iter().chain(&["../shared/src"]) {
        println!("cargo:rerun-if-changed={}", frontend.join(input).display());
    }
    if std::env::var_os("KICADMIUM_SKIP_FRONTEND_BUILD").is_some() {
        return;
    }

    let built = std::fs::metadata(&dist_index)
        .and_then(|m| m.modified())
        .ok();
    let source = INPUTS
        .iter()
        .map(|input| frontend.join(input))
        .chain([root.join("shared/src")])
        .filter_map(|path| newest(&path))
        .max();
    if matches!((built, source), (Some(b), Some(s)) if b >= s) {
        return;
    }

    let profile_flag = match std::env::var("PROFILE").as_deref() {
        Ok("release") => Some("--release"),
        _ => None,
    };
    let status = Command::new("trunk")
        .arg("build")
        .args(profile_flag)
        .current_dir(&frontend)
        .env("CARGO_TARGET_DIR", root.join("target/frontend"))
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .unwrap_or_else(|err| {
            panic!("running trunk to build frontend/dist failed ({err}); install it with `cargo install trunk --locked`")
        });
    assert!(status.success(), "trunk build failed: {status}");
}
