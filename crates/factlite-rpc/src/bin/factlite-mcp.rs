//! The MCP server: Model Context Protocol over stdio, on named stores.

use std::{
    io::{BufReader, Write},
    path::PathBuf,
    process::ExitCode,
};

use factlite_rpc::mcp::{Binding, McpOptions, McpServer, valid_store_name};

const HELP: &str = "Usage:
  factlite-mcp (--store NAME=PATH | --writable-store NAME=PATH)... [options]
  factlite-mcp --help | --version

Serves the Model Context Protocol over stdio on the named stores. Each store
is opened before the first message is read. Tools: head schema facts pull
lookup history transactions query, and transact when a store is writable.
See docs/mcp.md.

--store NAME=PATH            bind a store read-only
--writable-store NAME=PATH   bind a store that transact may write
--create                     create a writable store whose file is missing
--validation full|structural how read-only stores are opened (default full)
--busy-timeout-ms MS         lock wait for every store (default 2000)
--metadata KEY=VALUE         add this string to every transaction's metadata
--page-items N               most items in one page (default 200)
--page-bytes N               most bytes of items in one page (default 262144)
--max-line-bytes N           longest accepted line (default 16777216)

NAME is letters, digits, _ or -. Exit codes: 0 end of input, 2 invalid
arguments or a store that cannot be opened, 5 input or output failed.
";

enum Command {
    Help,
    Version,
    Serve(McpOptions),
}

fn number(value: &str, option: &str) -> Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{option} expects an unsigned decimal number"));
    }
    value
        .parse()
        .map_err(|_| format!("{option} is out of range"))
}

fn parse(args: Vec<std::ffi::OsString>) -> Result<Command, String> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    let mut options = McpOptions::default();
    let mut seen: Vec<String> = Vec::new();
    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        let option = option
            .into_string()
            .map_err(|_| "expected UTF-8 options".to_owned())?;
        let repeatable = matches!(
            option.as_str(),
            "--store" | "--writable-store" | "--metadata"
        );
        if !repeatable && seen.contains(&option) {
            return Err(format!("duplicate option: {option}"));
        }
        seen.push(option.clone());
        if option == "--create" {
            options.create = true;
            continue;
        }
        if !matches!(
            option.as_str(),
            "--store"
                | "--writable-store"
                | "--validation"
                | "--busy-timeout-ms"
                | "--metadata"
                | "--page-items"
                | "--page-bytes"
                | "--max-line-bytes"
        ) {
            return Err(format!("unknown option: {option}; use --help"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value: {option}"))?;
        match option.as_str() {
            "--store" | "--writable-store" => {
                let value = value
                    .into_string()
                    .map_err(|_| format!("{option} expects UTF-8"))?;
                let (name, path) = value
                    .split_once('=')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or_else(|| format!("{option} expects NAME=PATH"))?;
                if !valid_store_name(name) {
                    return Err(format!(
                        "{option}: store name {name:?} must be letters, digits, _ or -, at most 64 bytes"
                    ));
                }
                options.stores.push(Binding {
                    name: name.to_owned(),
                    path: PathBuf::from(path),
                    writable: option == "--writable-store",
                });
            }
            "--metadata" => {
                let value = value
                    .into_string()
                    .map_err(|_| "--metadata expects UTF-8".to_owned())?;
                let (key, text) = value
                    .split_once('=')
                    .ok_or("--metadata expects KEY=VALUE")?;
                if key.is_empty() {
                    return Err("--metadata expects a non-empty KEY".into());
                }
                if options.metadata.iter().any(|(seen, _)| seen == key) {
                    return Err(format!("--metadata key {key} is given twice"));
                }
                options.metadata.push((key.to_owned(), text.to_owned()));
            }
            _ => {
                let value = value
                    .into_string()
                    .map_err(|_| format!("{option} expects UTF-8"))?;
                match option.as_str() {
                    "--validation" => {
                        options.structural = match value.as_str() {
                            "full" => false,
                            "structural" => true,
                            _ => return Err("--validation expects full or structural".into()),
                        }
                    }
                    "--busy-timeout-ms" => {
                        options.busy_timeout_ms = Some(number(&value, &option)?);
                    }
                    "--page-items" | "--page-bytes" | "--max-line-bytes" => {
                        let count = usize::try_from(number(&value, &option)?)
                            .map_err(|_| format!("{option} is out of range"))?;
                        match option.as_str() {
                            "--page-items" if count == 0 => {
                                return Err("--page-items must be at least 1".into());
                            }
                            "--page-items" => options.page_items = count,
                            "--page-bytes" => options.page_bytes = count,
                            _ => options.max_line_bytes = count,
                        }
                    }
                    _ => unreachable!("options are matched above"),
                }
            }
        }
    }
    if options.stores.is_empty() {
        return Err("bind at least one store with --store or --writable-store".into());
    }
    if options.create && !options.stores.iter().any(|store| store.writable) {
        return Err("--create applies only to --writable-store".into());
    }
    Ok(Command::Serve(options))
}

fn main() -> ExitCode {
    let fail = |code: u8, message: &str| {
        let _ = writeln!(std::io::stderr(), "factlite-mcp: {message}");
        ExitCode::from(code)
    };
    match parse(std::env::args_os().skip(1).collect()) {
        Ok(Command::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("factlite-mcp {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Serve(options)) => match McpServer::start(options) {
            Err(message) => fail(2, &message),
            Ok(mut server) => match server.serve(
                BufReader::new(std::io::stdin().lock()),
                std::io::stdout().lock(),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => fail(5, &error.to_string()),
            },
        },
        Err(message) => fail(2, &message),
    }
}
