//! The stdio server: JSON-RPC requests on stdin, responses on stdout.

use std::{
    io::{BufReader, Write},
    process::ExitCode,
};

use factlite_rpc::{DispatchOptions, Dispatcher, ServerOptions, serve};

const HELP: &str = "Usage:
  factlite-rpc [--max-line-bytes N] [--require-idempotency-key]
  factlite-rpc --help | --version

Serves JSON-RPC 2.0 over stdio: one request per line on stdin, one response
per line on stdout, diagnostics on stderr. The process ends at end of input or
after a shutdown request.

--max-line-bytes N           longest accepted line (default 16777216)
--require-idempotency-key    refuse a transact with no key or an empty key

Methods: open close head verify transact schema facts pull lookup history
transactions query backup shutdown. See docs/rpc.md.
Exit codes: 0 end of input or shutdown, 2 invalid arguments, 5 input or
output failed.
";

enum Command {
    Help,
    Version,
    Serve(DispatchOptions, ServerOptions),
}

fn parse(args: Vec<std::ffi::OsString>) -> Result<Command, String> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    let mut dispatch = DispatchOptions::default();
    let mut server = ServerOptions::default();
    let mut seen = Vec::new();
    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| "expected UTF-8 options".to_owned())?;
        if seen.contains(&option) {
            return Err(format!("duplicate option: {option}"));
        }
        match option.as_str() {
            "--require-idempotency-key" => dispatch.require_idempotency_key = true,
            "--max-line-bytes" => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("missing value: --max-line-bytes")?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("--max-line-bytes expects unsigned decimal bytes".into());
                }
                server.max_line_bytes = value
                    .parse()
                    .map_err(|_| "--max-line-bytes is out of range".to_owned())?;
            }
            _ => return Err(format!("unknown option: {option}; use --help")),
        }
        seen.push(option);
    }
    Ok(Command::Serve(dispatch, server))
}

/// Writes text to stdout, exiting 5 if stdout is closed.
fn print(text: &str) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "factlite-rpc: {error}");
            ExitCode::from(5)
        }
    }
}

fn main() -> ExitCode {
    match parse(std::env::args_os().skip(1).collect()) {
        Ok(Command::Help) => print(HELP),
        Ok(Command::Version) => print(&format!("factlite-rpc {}\n", env!("CARGO_PKG_VERSION"))),
        Ok(Command::Serve(dispatch, server)) => {
            let mut dispatcher = Dispatcher::new(dispatch);
            let served = serve(
                BufReader::new(std::io::stdin().lock()),
                std::io::stdout().lock(),
                std::io::stderr(),
                &mut dispatcher,
                &server,
            );
            match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = writeln!(std::io::stderr(), "factlite-rpc: {error}");
                    ExitCode::from(5)
                }
            }
        }
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "factlite-rpc: {message}");
            ExitCode::from(2)
        }
    }
}
