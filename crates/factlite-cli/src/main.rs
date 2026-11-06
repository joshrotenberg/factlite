//! Thin command-line parsing and rendering around Factlite's public APIs.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use factlite::{Db, ErrorCode, FactFilter, Revision, Store, StoreOptions, Validation, wire};
use serde_json::json;

const HELP: &str = "Usage:
  factlite init PATH [--busy-timeout-ms MS]
  factlite --db PATH verify
  factlite --db PATH backup --to PATH
  factlite --db PATH transact --file PATH|-
  factlite --db PATH pull --entity ID --attrs ATTR[,ATTR...] [VIEW]
  factlite --db PATH lookup --attr ATTR --value TAGGED_JSON [VIEW]
  factlite --db PATH schema [VIEW]
  factlite --db PATH facts [FILTER] [VIEW]
  factlite --db PATH history [FILTER] [--after REVISION] [VIEW]
  factlite --db PATH transactions [--after REVISION] [VIEW]
  factlite --db PATH query --file PATH|- [VIEW]
  factlite --help | --version

VIEW: --at SNAPSHOT_TOKEN or --at-file PATH|- (otherwise capture head once)
FILTER: --entity ID, --attr ATTR, --value TAGGED_JSON
--after defaults to 0. Empty --attrs projects no fields.
Read commands accept --validation full|structural (default full). Structural
skips replaying the log at open; run verify to validate fully.
Every database command also accepts --busy-timeout-ms MS: how long to wait for
a lock held by another process before failing busy (default 2000; 0 fails at
once).
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
    Init(PathBuf, StoreOptions),
    Database {
        path: PathBuf,
        command: String,
        store: StoreOptions,
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
        return Ok(Command::Init(
            PathBuf::from(&args[1]),
            StoreOptions::default(),
        ));
    }
    if args.len() == 4 && args[0] == "init" && args[2] == "--busy-timeout-ms" {
        return Ok(Command::Init(
            PathBuf::from(&args[1]),
            store_options(Some(args[3].clone()))?,
        ));
    }
    if args.first().is_some_and(|arg| arg == "init") {
        return Err(invalid("init expects PATH [--busy-timeout-ms MS]"));
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
        "verify" => &[],
        "backup" => &["--to"],
        "transact" => &["--file"],
        "query" => &["--file", "--at", "--at-file"],
        "pull" => &["--entity", "--attrs", "--at", "--at-file"],
        "lookup" => &["--attr", "--value", "--at", "--at-file"],
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
        let read_command = matches!(
            command.as_str(),
            "schema" | "facts" | "pull" | "lookup" | "history" | "transactions" | "query"
        );
        if option != "--busy-timeout-ms"
            && !(option == "--validation" && read_command)
            && !allowed.contains(&option.as_str())
        {
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
        "backup" => &["--to"],
        "transact" | "query" => &["--file"],
        "pull" => &["--entity", "--attrs"],
        "lookup" => &["--attr", "--value"],
        _ => &[],
    };
    for option in required {
        if !options.contains_key(*option) {
            return Err(invalid(format!("missing option: {option}")));
        }
    }
    // Validated here, with the other options, before any input is read.
    let mut store = store_options(options.remove("--busy-timeout-ms"))?;
    if let Some(validation) = options.remove("--validation") {
        store = store.with_validation(match text(validation)?.as_str() {
            "full" => Validation::Full,
            "structural" => Validation::Structural,
            _ => return Err(invalid("--validation expects full or structural")),
        });
    }
    Ok(Command::Database {
        path,
        command,
        store,
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
/// Connection settings from `--busy-timeout-ms`, validated by the library.
fn store_options(timeout: Option<OsString>) -> Result<StoreOptions> {
    let Some(timeout) = timeout else {
        return Ok(StoreOptions::default());
    };
    let millis = text(timeout)?;
    if millis.is_empty() || !millis.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(
            "--busy-timeout-ms expects unsigned decimal milliseconds",
        ));
    }
    let millis: u64 = millis
        .parse()
        .map_err(|_| invalid("--busy-timeout-ms is out of range"))?;
    Ok(StoreOptions::new().with_busy_timeout(std::time::Duration::from_millis(millis))?)
}
fn open_existing(path: &Path, options: &StoreOptions) -> Result<Store> {
    // CLI creation is explicit, even though the library's Store::open can initialize.
    std::fs::metadata(path).map_err(|error| Failure::Io("open database", error))?;
    Ok(Store::open_with(path, options)?)
}
fn run(command: Command) -> Result<String> {
    match command {
        Command::Help => Ok(HELP.into()),
        Command::Version => Ok(format!("factlite {}", env!("CARGO_PKG_VERSION"))),
        Command::Init(path, store) => Ok(wire::encode_snapshot(
            &Store::open_with(path, &store)?.head()?,
        )),
        Command::Database {
            path,
            command,
            store,
            mut options,
        } => {
            if command == "verify" {
                // Read-only, so verifying never takes the write lock.
                let db =
                    Db::open_read_only_with(&path, &store.with_validation(Validation::Structural))?;
                return Ok(wire::encode_snapshot(&db.verify()?));
            }
            if command == "backup" {
                let destination =
                    PathBuf::from(options.remove("--to").expect("required during parse"));
                return Ok(wire::encode_snapshot(
                    &open_existing(&path, &store)?.backup(destination)?,
                ));
            }
            if command == "transact" {
                let file = PathBuf::from(options.remove("--file").expect("required during parse"));
                let request = wire::decode_tx(&read_input(&file)?)?;
                return Ok(wire::encode_report(
                    &open_existing(&path, &store)?.commit(request)?,
                ));
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
            let db = match at {
                Some(snapshot) => Db::open_read_only_with(&path, &store)?.at(&snapshot)?,
                None => Db::open_read_only_with(&path, &store)?,
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
                "lookup" => {
                    let attribute = filter.attribute.as_ref().expect("required during parse");
                    let value = filter.value.as_ref().expect("required during parse");
                    wire::encode_lookup(db.lookup(attribute, value)?.as_ref())
                }
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
