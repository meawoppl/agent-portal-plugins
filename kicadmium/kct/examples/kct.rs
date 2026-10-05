//! Standalone `kct` driver for parity checks without building the kicadmium
//! backend (which embeds the frontend): `cargo run -p kct --example kct -- <args>`.

fn main() {
    match kct::cli::run(std::env::args_os().skip(1)) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("Error: {err:?}");
            std::process::exit(1);
        }
    }
}
