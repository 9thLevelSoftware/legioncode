#![forbid(unsafe_code)]
//! Binary entrypoint for the `t3` Front-Door CLI and server.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = t3_cli::run_cli_blocking(&args);
    std::process::exit(code);
}
