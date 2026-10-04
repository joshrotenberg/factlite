//! Thin command-line parsing and rendering around Factlite's public APIs.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use factlite::{ErrorCode, FactFilter, Revision, Store, wire};
use serde_json::json;

const HELP: &str = "Usage:
  factlite init PATH
  factlite --db PATH transact --file PATH|-
  factlite --db PATH pull --entity ID --attrs ATTR[,ATTR...] [VIEW]
  factlite --db PATH schema [VIEW]
  factlite --db PATH facts [FILTER] [VIEW]
  factlite --db PATH history [FILTER] [--after REVISION] [VIEW]
  factlite --db PATH transactions [--after REVISION] [VIEW]
  factlite --db PATH query --file PATH|- [VIEW]
  factlite --help | --version

VIEW: --at SNAPSHOT_TOKEN or --at-file PATH|- (otherwise capture head once)
FILTER: --entity ID, --attr ATTR, --value TAGGED_JSON
--after defaults to 0. Empty --attrs projects no fields.
Database commands emit one versioned JSON result. Errors are JSON on stderr.
Exit codes: 0 success, 2 invalid input, 3 conflict, 4 busy, 5 storage.
";

enum Failure {
    Library(factlite::Error),
    Invalid(String),
    Io(&'static str, std::io::Error),
}
impl From<factlite::Error> for Failure {
    fn from(error: factlite::Error) -> Self {
        Self::Library(error)
    }
}
impl Failure {
    fn render(&self) -> (u8, String) {
        match self {
            Self::Library(error) => {
                let exit = match error.code() {
                    ErrorCode::ConflictingBatch | ErrorCode::UniqueViolation | ErrorCode::PreconditionFailed | ErrorCode::HeadMismatch | ErrorCode::IdempotencyConflict => 3,
                    ErrorCode::Busy => 4,
                    ErrorCode::Storage | ErrorCode::UnsupportedFormat => 5,
                    _ => 2,
                };
                (exit,wire::encode_error(error))
            }
            Self::Invalid(message) => (2,json!({"version":1,"error":{"code":"InvalidRequest","message":message,"context":{"field":"arguments"}}}).to_string()),
            Self::Io(operation,error) => (5,json!({"version":1,"error":{"code":"Storage","message":error.to_string(),"context":{"operation":operation}}}).to_string()),
        }
    }
}
type Result<T> = std::result::Result<T, Failure>;
fn invalid(message: impl Into<String>) -> Failure {
    Failure::Invalid(message.into())
}
fn text(value: OsString) -> Result<String> {
    value
        .into_string()
        .map_err(|_| invalid("expected UTF-8 option or value"))
}
fn typed<T: std::str::FromStr<Err = factlite::Error>>(value: OsString) -> Result<T> {
    Ok(text(value)?.parse()?)
}

enum Command {
    Help,
    Version,
    Init(PathBuf),
    Database {
        path: PathBuf,
        command: String,
        options: BTreeMap<String, OsString>,
    },
}
fn parse(args: Vec<OsString>) -> Result<Command> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    if args.len() == 2 && args[0] == "init" {
        return Ok(Command::Init(PathBuf::from(&args[1])));
    }
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--db")) {
        return Err(invalid(
            "expected --db PATH COMMAND or init PATH; use --help",
        ));
    }
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| invalid("missing database path"))?,
    );
    let command = text(
        args.next()
            .ok_or_else(|| invalid("missing database command"))?,
    )?;
    let allowed: &[&str] = match command.as_str() {
        "transact" => &["--file"],
        "query" => &["--file", "--at", "--at-file"],
        "pull" => &["--entity", "--attrs", "--at", "--at-file"],
        "schema" => &["--at", "--at-file"],
        "facts" => &["--entity", "--attr", "--value", "--at", "--at-file"],
        "history" => &[
            "--entity",
            "--attr",
            "--value",
            "--after",
            "--at",
            "--at-file",
        ],
        "transactions" => &["--after", "--at", "--at-file"],
        _ => return Err(invalid(format!("unknown command: {command}"))),
    };
    let mut options = BTreeMap::new();
    while let Some(option) = args.next() {
        let option = text(option)?;
        if !allowed.contains(&option.as_str()) {
            return Err(invalid(format!("unknown option for {command}: {option}")));
        }
        let value = args
            .next()
            .ok_or_else(|| invalid(format!("missing value: {option}")))?;
        if options.insert(option.clone(), value).is_some() {
            return Err(invalid(format!("duplicate option: {option}")));
        }
    }
    if options.contains_key("--at") && options.contains_key("--at-file") {
        return Err(invalid("--at and --at-file are mutually exclusive"));
    }
    if options.get("--file").is_some_and(|value| value == "-")
        && options.get("--at-file").is_some_and(|value| value == "-")
    {
        return Err(invalid("only one input may consume stdin"));
    }
    let required: &[&str] = match command.as_str() {
        "transact" | "query" => &["--file"],
        "pull" => &["--entity", "--attrs"],
        _ => &[],
    };
    for option in required {
        if !options.contains_key(*option) {
            return Err(invalid(format!("missing option: {option}")));
        }
    }
    Ok(Command::Database {
        path,
        command,
        options,
    })
}
fn read_input(path: &Path) -> Result<String> {
    let bytes = if path == Path::new("-") {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut bytes)
            .map_err(|error| Failure::Io("read stdin", error))?;
        bytes
    } else {
        std::fs::read(path).map_err(|error| Failure::Io("read input file", error))?
    };
    String::from_utf8(bytes).map_err(|_| invalid("JSON input must be UTF-8"))
}
fn open_existing(path: &Path) -> Result<Store> {
    // CLI creation is explicit, even though the library's Store::open can initialize.
    std::fs::metadata(path).map_err(|error| Failure::Io("open database", error))?;
    Ok(Store::open(path)?)
}
fn run(command: Command) -> Result<String> {
    match command {
        Command::Help => Ok(HELP.into()),
        Command::Version => Ok(format!("factlite {}", env!("CARGO_PKG_VERSION"))),
        Command::Init(path) => Ok(wire::encode_snapshot(&Store::open(path)?.head()?)),
        Command::Database {
            path,
            command,
            mut options,
        } => {
            if command == "transact" {
                let file = PathBuf::from(options.remove("--file").expect("required during parse"));
                let request = wire::decode_tx(&read_input(&file)?)?;
                return Ok(wire::encode_report(&open_existing(&path)?.commit(request)?));
            }
            let at = if let Some(file) = options.remove("--at-file") {
                Some(wire::decode_snapshot(&read_input(&PathBuf::from(file))?)?)
            } else {
                options.remove("--at").map(typed).transpose()?
            };
            let query = options
                .remove("--file")
                .map(|file| -> Result<_> {
                    Ok(wire::decode_query(&read_input(&PathBuf::from(file))?)?)
                })
                .transpose()?;
            let entity = options.remove("--entity").map(typed).transpose()?;
            let attribute = options.remove("--attr").map(typed).transpose()?;
            let value = options
                .remove("--value")
                .map(|value| -> Result<_> { Ok(wire::decode_value(&text(value)?)?) })
                .transpose()?;
            let after = options
                .remove("--after")
                .map(typed)
                .transpose()?
                .unwrap_or(Revision::ZERO);
            let attrs = options
                .remove("--attrs")
                .map(|value| {
                    let value = text(value)?;
                    if value.is_empty() {
                        return Ok(Vec::new());
                    }
                    value
                        .split(',')
                        .map(|name| Ok(name.parse()?))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;
            let store = open_existing(&path)?;
            let db = match at {
                Some(snapshot) => store.db_at(&snapshot)?,
                None => store.db()?,
            };
            let filter = FactFilter {
                entity: entity.clone(),
                attribute,
                value,
            };
            Ok(match command.as_str() {
                "query" => {
                    let (query, params) = query.expect("required during parse");
                    wire::encode_query_result(&db.query(&query, &params)?)
                }
                "schema" => wire::encode_schema(&db.schema()?),
                "facts" => wire::encode_facts(&db.facts(filter)?),
                "pull" => wire::encode_entity(
                    db.pull(
                        &entity.expect("required during parse"),
                        &attrs.expect("required during parse"),
                    )?
                    .as_ref(),
                ),
                "history" => wire::encode_history(&db.history(filter, after)?),
                "transactions" => wire::encode_transactions(&db.transactions(after)?),
                _ => unreachable!("command validated during parse"),
            })
        }
    }
}
fn main() -> ExitCode {
    let result = parse(std::env::args_os().skip(1).collect())
        .and_then(run)
        .and_then(|mut output| {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            std::io::stdout()
                .lock()
                .write_all(output.as_bytes())
                .map_err(|error| Failure::Io("write stdout", error))
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let (exit, output) = error.render();
            let _ = writeln!(std::io::stderr().lock(), "{output}");
            ExitCode::from(exit)
        }
    }
}
