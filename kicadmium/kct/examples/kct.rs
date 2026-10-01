//! Standalone `kct` entry point for parity runs:
//! `cargo run --release -p kct --example kct -- route board.kicad_pcb ...`.

fn main() {
    match kct::cli::run(std::env::args_os().skip(1)) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}
