//! Thin `kicadmium` binary: embed the built Yew frontend and hand off to the
//! `backend` library. Keep this file tiny; see the note at the top of lib.rs.
//! `build.rs` generates the asset table expression included here.

fn main() -> anyhow::Result<()> {
    backend::run(include!(concat!(env!("OUT_DIR"), "/frontend_assets.rs")))
}
