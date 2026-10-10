//! Builds the Yew frontend with trunk when `frontend/dist` is missing or older
//! than its inputs, so `cargo build -p backend` alone yields a working binary,
//! then turns `frontend/dist` into an asset table the `kicadmium` binary embeds
//! with `include_bytes!`. Trunk gets its own target dir: the outer cargo holds
//! the workspace one. Set `KICADMIUM_SKIP_FRONTEND_BUILD=1` to embed whatever
//! dist exists.
//!
//! Embedding happens here rather than in a proc macro on purpose. A macro has
//! to emit the files as byte-string literals in its token stream, and rustc
//! spent close to a minute digesting the three megabytes of this frontend that
//! way on every build of the binary. `include_bytes!` of files already on disk
//! compiles in about a second, and the brotli pre-compression done here is
//! cached by cargo like any other build-script output.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

/// Brotli quality for the embedded assets; `KICADMIUM_BROTLI_QUALITY` overrides.
/// Measured on this frontend (3.2 MB, mostly the wasm): quality 11 takes 26 s
/// for 0.70 MB, quality 9 takes under 2 s for 0.79 MB. The extra 90 KB is
/// fetched once a week per client; the 24 s are paid on every build.
const BROTLI_QUALITY: u32 = 9;

const INPUTS: &[&str] = &[
    "src",
    "static",
    "index.html",
    "style.css",
    "panels.css",
    "touch-views.css",
    "Trunk.toml",
    "Cargo.toml",
];

/// Content types worth pre-compressing; images and fonts are already compressed.
const COMPRESSIBLE: &[&str] = &[
    "text/html",
    "text/css",
    "text/plain",
    "text/markdown",
    "text/javascript",
    "application/javascript",
    "application/json",
    "application/xml",
    "text/xml",
    "image/svg+xml",
    "application/wasm",
];

fn newest(path: &Path) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    fs::read_dir(path)
        .ok()?
        .filter_map(|entry| newest(&entry.ok()?.path()))
        .max()
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap();
    let frontend = root.join("frontend");
    let dist = frontend.join("dist");

    println!("cargo:rerun-if-env-changed=KICADMIUM_SKIP_FRONTEND_BUILD");
    println!("cargo:rerun-if-env-changed=KICADMIUM_BROTLI_QUALITY");
    // Deleting dist must trigger a rebuild too; the directory itself is scanned
    // so a changed asset re-generates the table.
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );
    println!("cargo:rerun-if-changed={}", dist.display());
    for input in INPUTS.iter().chain(&["../shared/src"]) {
        println!("cargo:rerun-if-changed={}", frontend.join(input).display());
    }
    if std::env::var_os("KICADMIUM_SKIP_FRONTEND_BUILD").is_none() {
        build_frontend(root, &frontend, &dist);
    }
    embed(&dist);
}

fn build_frontend(root: &Path, frontend: &Path, dist: &Path) {
    let built = fs::metadata(dist.join("index.html"))
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
        .current_dir(frontend)
        .env("CARGO_TARGET_DIR", root.join("target/frontend"))
        // Trunk 0.21 interprets NO_COLOR's conventional `1` as its boolean
        // --no-color option and rejects it. Do not leak that outer UI setting.
        .env_remove("NO_COLOR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .unwrap_or_else(|err| {
            panic!("running trunk to build frontend/dist failed ({err}); install it with `cargo install trunk --locked`")
        });
    assert!(status.success(), "trunk build failed: {status}");
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).expect("read frontend/dist") {
        let path = entry.expect("read dist entry").path();
        if path.is_dir() {
            walk(base, &path, out);
        } else {
            let route = path
                .strip_prefix(base)
                .unwrap()
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => s.to_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            out.push((format!("/{route}"), path));
        }
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn content_type(path: &Path) -> String {
    path.extension()
        .and_then(|ext| mime_guess::from_ext(&ext.to_string_lossy()).first_raw())
        .unwrap_or("application/octet-stream")
        .to_owned()
}

fn brotli(bytes: &[u8]) -> Option<Vec<u8>> {
    let quality = std::env::var("KICADMIUM_BROTLI_QUALITY")
        .ok()
        .and_then(|q| q.parse().ok())
        .unwrap_or(BROTLI_QUALITY);
    let mut writer = brotli::CompressorWriter::new(Vec::new(), 4096, quality, 22);
    writer.write_all(bytes).ok()?;
    let compressed = writer.into_inner();
    (compressed.len() < bytes.len()).then_some(compressed)
}

/// Write `$OUT_DIR/frontend_assets.rs`: an expression evaluating to
/// `&'static [backend::EmbeddedAsset]` for everything under `dist`.
fn embed(dist: &Path) {
    assert!(
        dist.join("index.html").is_file(),
        "{} has no index.html; run `trunk build` in frontend/ or unset KICADMIUM_SKIP_FRONTEND_BUILD",
        dist.display()
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let store = out.join("frontend-assets");
    let _ = fs::remove_dir_all(&store);
    fs::create_dir_all(&store).expect("create asset store");

    let mut files = Vec::new();
    walk(dist, dist, &mut files);
    files.sort();

    // Brotli at quality 11 is slow enough to parallelise across files.
    let entries: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(index, (route, path))| {
                let store = &store;
                scope.spawn(move || {
                    let bytes = fs::read(path).expect("read dist file");
                    let content_type = content_type(path);
                    let etag = format!("\"{:016x}-{:x}\"", fnv1a64(&bytes), bytes.len());
                    let brotli = COMPRESSIBLE
                        .contains(&content_type.as_str())
                        .then(|| brotli(&bytes))
                        .flatten()
                        .map(|compressed| {
                            let file = store.join(format!("{index}.br"));
                            fs::write(&file, compressed).expect("write brotli asset");
                            format!("Some(include_bytes!({:?}))", file.display().to_string())
                        })
                        .unwrap_or_else(|| "None".to_owned());
                    format!(
                        "    ::backend::EmbeddedAsset {{ route: {route:?}, content_type: {content_type:?}, etag: {etag:?}, bytes: include_bytes!({:?}), brotli: {brotli} }},\n",
                        path.display().to_string()
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("asset worker"))
            .collect()
    });

    let mut source = String::from("{\n    static ASSETS: &[::backend::EmbeddedAsset] = &[\n");
    for entry in entries {
        source.push_str(&entry);
    }
    source.push_str("    ];\n    ASSETS\n}\n");
    fs::write(out.join("frontend_assets.rs"), source).expect("write asset table");
}
