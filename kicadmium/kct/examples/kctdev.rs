fn main() {
    let code = match kct::cli::run(std::env::args_os().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("kct: error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}
