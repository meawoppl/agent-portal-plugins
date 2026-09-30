fn main() {
    let code = pcb_lint::cli::run(std::env::args_os().skip(1)).unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        1
    });
    std::process::exit(code);
}
