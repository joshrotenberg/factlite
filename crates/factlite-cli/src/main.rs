//! Command-line entry point for Factlite.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        println!("factlite {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 && args[0] == "--help" {
        println!("Usage: factlite --help | --version");
        println!("Database commands are not implemented yet.");
        return ExitCode::SUCCESS;
    }

    eprintln!("Database commands are not implemented yet. Use --help or --version.");
    ExitCode::from(2)
}
