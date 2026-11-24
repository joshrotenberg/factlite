//! The Model Context Protocol front end: named stores, nine tools, pages.
//!
//! Every tool call becomes one call to the [`Dispatcher`]. This module adds
//! what a model needs around that call and nothing about storage.

use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json, value::RawValue};

use crate::{
    dispatch::{DispatchOptions, Dispatcher, Failure, Reply},
    json::{self, Kind, Malformed, Members, Top},
    server::{self, Line},
};

/// The protocol revision that carries its version in every request.
pub const STATELESS_REVISION: &str = "2026-07-28";
/// The protocol revisions that open with `initialize`, oldest first.
pub const HANDSHAKE_REVISIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const SERVER_NAME: &str = "factlite-mcp";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const RESOURCE_NOT_FOUND_HANDSHAKE: i64 = -32002;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// How long a client may reuse a list that cannot change while this process
/// lives, in milliseconds.
const FIXED_LIST_TTL_MS: u64 = 3_600_000;

/// One store named on the command line.
#[derive(Clone, Debug)]
pub struct Binding {
    /// The name tools use for the store.
    pub name: String,
    /// The store file.
    pub path: PathBuf,
    /// Whether `transact` may write it.
    pub writable: bool,
}

/// Settings for [`McpServer`].
#[derive(Clone, Debug)]
pub struct McpOptions {
    /// The stores to open, in the order named.
    pub stores: Vec<Binding>,
    /// Create a writable store whose file is missing.
    pub create: bool,
    /// Open read-only stores with structural validation.
    pub structural: bool,
    /// The busy timeout of every connection, or None for the library's.
    pub busy_timeout_ms: Option<u64>,
    /// String metadata added to every transaction.
    pub metadata: Vec<(String, String)>,
    /// The most items one page holds.
    pub page_items: usize,
    /// The most bytes of items one page holds, beyond its first item.
    pub page_bytes: usize,
    /// The longest accepted line.
    pub max_line_bytes: usize,
}
impl Default for McpOptions {
    fn default() -> Self {
        Self {
            stores: Vec::new(),
            create: false,
            structural: false,
            busy_timeout_ms: None,
            metadata: Vec::new(),
            page_items: 200,
            page_bytes: 256 * 1024,
            max_line_bytes: server::DEFAULT_MAX_LINE_BYTES,
        }
    }
}

/// Whether a store name can be used: letters, digits, `_` and `-`, at most
/// 64 bytes. Names appear in resource URIs and tool schemas unescaped.
pub fn valid_store_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

struct Bound {
    name: String,
    handle: String,
    writable: bool,
}

/// A tool: its dispatch method and what the front end does around it.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Tool {
    Head,
    Schema,
    Facts,
    Pull,
    Lookup,
    History,
    Transactions,
    Query,
    Transact,
}
impl Tool {
    const ALL: [Tool; 9] = [
        Tool::Head,
        Tool::Schema,
        Tool::Facts,
        Tool::Pull,
        Tool::Lookup,
        Tool::History,
        Tool::Transactions,
        Tool::Query,
        Tool::Transact,
    ];
    /// The tool's name, which is also its dispatch method.
    fn name(self) -> &'static str {
        match self {
            Tool::Head => "head",
            Tool::Schema => "schema",
            Tool::Facts => "facts",
            Tool::Pull => "pull",
            Tool::Lookup => "lookup",
            Tool::History => "history",
            Tool::Transactions => "transactions",
            Tool::Query => "query",
            Tool::Transact => "transact",
        }
    }
    /// The member of the wire output that holds the items of a paged read.
    fn items(self) -> Option<&'static str> {
        match self {
            Tool::Schema => Some("schema"),
            Tool::Facts => Some("facts"),
            Tool::History => Some("events"),
            Tool::Transactions => Some("transactions"),
            Tool::Query => Some("rows"),
            Tool::Head | Tool::Pull | Tool::Lookup | Tool::Transact => None,
        }
    }
}

/// Which form of the protocol a request is answered in.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Era {
    /// Opened with `initialize`; results carry only their own members.
    Handshake,
    /// The request named [`STATELESS_REVISION`] in its `_meta`.
    Stateless,
}

/// A JSON-RPC error to send.
struct ProtocolError {
    code: i64,
    message: String,
    data: Option<Json>,
}
impl ProtocolError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    fn params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}

/// An MCP server over the stores it was started with.
pub struct McpServer {
    dispatcher: Dispatcher,
    stores: Vec<Bound>,
    options: McpOptions,
}

impl McpServer {
    /// Opens every bound store. The error names the store that failed.
    pub fn start(options: McpOptions) -> Result<Self, String> {
        if options.stores.is_empty() {
            return Err("bind at least one store with --store or --writable-store".into());
        }
        let mut dispatcher = Dispatcher::new(DispatchOptions {
            require_idempotency_key: true,
        });
        let mut stores: Vec<Bound> = Vec::new();
        for binding in &options.stores {
            if !valid_store_name(&binding.name) {
                return Err(format!(
                    "store name {:?} must be letters, digits, _ or -, at most 64 bytes",
                    binding.name
                ));
            }
            if stores.iter().any(|bound| bound.name == binding.name) {
                return Err(format!("store name {} is bound twice", binding.name));
            }
            let mut params = json!({"path": binding.path});
            if binding.writable {
                params["create"] = json!(options.create);
            } else {
                params["read_only"] = json!(true);
                if options.structural {
                    params["validation"] = json!("structural");
                }
            }
            if let Some(millis) = options.busy_timeout_ms {
                params["busy_timeout_ms"] = json!(millis);
            }
            let opened = dispatcher
                .call("open", Some(&params.to_string()))
                .map_err(|failure| {
                    format!(
                        "cannot open store {} at {}: {}",
                        binding.name,
                        binding.path.display(),
                        failure.message()
                    )
                })?;
            let opened: Json = serde_json::from_str(&opened).expect("open returns JSON");
            stores.push(Bound {
                name: binding.name.clone(),
                handle: opened["handle"].as_str().expect("a handle").to_owned(),
                writable: binding.writable,
            });
        }
        Ok(Self {
            dispatcher,
            stores,
            options,
        })
    }

    /// Serves MCP requests from `input` until end of input. Each line is one
    /// message and each response one line, flushed before the next is read.
    /// Returns an error only when `input` or `output` fails.
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        loop {
            let response = match server::read_line(&mut input, self.options.max_line_bytes)? {
                Line::End => return Ok(()),
                Line::TooLong => Some(server::failure(
                    "null",
                    PARSE_ERROR,
                    &format!("line is longer than {} bytes", self.options.max_line_bytes),
                    None,
                )),
                Line::Bytes(bytes) if server::is_blank(&bytes) => None,
                Line::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(text) => self.message(text),
                    Err(_) => Some(server::failure(
                        "null",
                        PARSE_ERROR,
                        "line is not UTF-8",
                        None,
                    )),
                },
            };
            if let Some(mut line) = response {
                line.push('\n');
                output.write_all(line.as_bytes())?;
                output.flush()?;
            }
        }
    }

    /// Handles one line. Returns the line to send, if any.
    fn message(&mut self, text: &str) -> Option<String> {
        let invalid = |message: &str| Some(server::failure("null", INVALID_REQUEST, message, None));
        let members = match Top::parse(text) {
            Ok(Top::Object(members)) => members,
            Ok(Top::Array(_)) => return invalid("batches are not supported"),
            Ok(Top::Scalar) => return invalid("a request must be an object"),
            Err(Malformed::Duplicate(message)) => return invalid(&message),
            Err(Malformed::Syntax(message)) => {
                return Some(server::failure("null", PARSE_ERROR, &message, None));
            }
        };
        let request = match server::request(members) {
            Ok(request) => request,
            Err(message) => return invalid(&message),
        };
        // A notification is never answered. None of them changes anything
        // here: there is no session to initialize and no call to cancel
        // while this one is running.
        let id = request.id?;
        if json::kind(&id) == Kind::Null {
            return invalid("id must be a string or a number");
        }
        Some(
            match self.respond(&request.method, request.params.as_deref()) {
                Ok(result) => server::success(id.get(), &result.to_string()),
                Err(error) => server::failure(
                    id.get(),
                    error.code,
                    &error.message,
                    error.data.map(|data| data.to_string()).as_deref(),
                ),
            },
        )
    }

    fn respond(&mut self, method: &str, params: Option<&RawValue>) -> Result<Json, ProtocolError> {
        // MCP adds members between revisions, so unknown ones are ignored
        // here. Tool arguments are strict; see `call`.
        let mut params = match params {
            None => Members::default(),
            Some(raw) => match Top::parse(raw.get()) {
                Ok(Top::Object(members)) => members,
                Ok(_) => return Err(ProtocolError::params("params must be an object")),
                Err(Malformed::Duplicate(message) | Malformed::Syntax(message)) => {
                    return Err(ProtocolError::params(message));
                }
            },
        };
        let era = self.era(&mut params)?;
        let mut result = match method {
            "initialize" => {
                let requested = params
                    .take("protocolVersion")
                    .as_deref()
                    .and_then(json::string)
                    .ok_or_else(|| ProtocolError::params("protocolVersion must be a string"))?;
                let version = HANDSHAKE_REVISIONS
                    .into_iter()
                    .find(|revision| *revision == requested)
                    .unwrap_or(HANDSHAKE_REVISIONS[HANDSHAKE_REVISIONS.len() - 1]);
                // The handshake form whatever the request carried.
                return Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": server_info(),
                    "instructions": self.instructions(),
                }));
            }
            "server/discover" => {
                if era != Era::Stateless {
                    return Err(ProtocolError::params(format!(
                        "server/discover needs params._meta with {PROTOCOL_VERSION}"
                    )));
                }
                let mut versions = vec![STATELESS_REVISION];
                versions.extend(HANDSHAKE_REVISIONS.into_iter().rev());
                cached(
                    json!({
                        "supportedVersions": versions,
                        "capabilities": {"tools": {}, "resources": {}},
                        "instructions": self.instructions(),
                    }),
                    FIXED_LIST_TTL_MS,
                )
            }
            "ping" => json!({}),
            "tools/list" => cached(json!({"tools": self.tools()}), FIXED_LIST_TTL_MS),
            "tools/call" => {
                let name = params
                    .take("name")
                    .as_deref()
                    .and_then(json::string)
                    .ok_or_else(|| ProtocolError::params("name must be a string"))?;
                let arguments = match params.take("arguments") {
                    None => Members::default(),
                    Some(raw) => match Top::parse(raw.get()) {
                        Ok(Top::Object(members)) => members,
                        Ok(_) => return Err(ProtocolError::params("arguments must be an object")),
                        // A repeated argument is the model's mistake to read.
                        Err(Malformed::Duplicate(message) | Malformed::Syntax(message)) => {
                            return Ok(self.finish(era, tool_error(&argument_error(&message))));
                        }
                    },
                };
                let tool = Tool::ALL
                    .into_iter()
                    .find(|tool| tool.name() == name)
                    .filter(|tool| *tool != Tool::Transact || self.any_writable())
                    .ok_or_else(|| ProtocolError::params(format!("Unknown tool: {name}")))?;
                match self.call(tool, arguments) {
                    Ok(text) => {
                        json!({"content": [{"type": "text", "text": text}], "isError": false})
                    }
                    Err(failure) => tool_error(&failure_json(&failure)),
                }
            }
            "resources/list" => cached(
                json!({"resources": self.stores.iter().map(|store| json!({
                    "uri": schema_uri(&store.name),
                    "name": format!("{}-schema", store.name),
                    "description": format!(
                        "Attribute definitions of the Factlite store {}, at its latest revision.",
                        store.name
                    ),
                    "mimeType": "application/json",
                })).collect::<Vec<_>>()}),
                FIXED_LIST_TTL_MS,
            ),
            "resources/templates/list" => {
                cached(json!({"resourceTemplates": []}), FIXED_LIST_TTL_MS)
            }
            "resources/read" => {
                let uri = params
                    .take("uri")
                    .as_deref()
                    .and_then(json::string)
                    .ok_or_else(|| ProtocolError::params("uri must be a string"))?;
                let not_found = || ProtocolError {
                    code: match era {
                        Era::Handshake => RESOURCE_NOT_FOUND_HANDSHAKE,
                        Era::Stateless => INVALID_PARAMS,
                    },
                    message: "Resource not found".into(),
                    data: Some(json!({"uri": uri})),
                };
                let handle = self
                    .stores
                    .iter()
                    .find(|store| schema_uri(&store.name) == uri)
                    .map(|store| store.handle.clone())
                    .ok_or_else(not_found)?;
                let text = self
                    .dispatcher
                    .call("schema", Some(&json!({"handle": handle}).to_string()))
                    .map_err(|failure| ProtocolError {
                        code: -32603,
                        message: failure.message(),
                        data: Some(failure_json(&failure)),
                    })?;
                // A commit can add an attribute at any time.
                cached(
                    json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": text}]}),
                    0,
                )
            }
            _ => {
                return Err(ProtocolError::new(
                    METHOD_NOT_FOUND,
                    format!("method not found: {method}"),
                ));
            }
        };
        if era == Era::Handshake {
            // The cache hints belong to the stateless revision.
            let object = result.as_object_mut().expect("results are objects");
            object.remove("ttlMs");
            object.remove("cacheScope");
        }
        Ok(self.finish(era, result))
    }

    /// Reads the era out of `params._meta`.
    fn era(&self, params: &mut Members) -> Result<Era, ProtocolError> {
        let Some(meta) = params.take("_meta") else {
            return Ok(Era::Handshake);
        };
        let mut meta = match Top::parse(meta.get()) {
            Ok(Top::Object(members)) => members,
            _ => return Err(ProtocolError::params("_meta must be an object")),
        };
        let Some(version) = meta.take(PROTOCOL_VERSION) else {
            return Ok(Era::Handshake);
        };
        let version = json::string(&version)
            .ok_or_else(|| ProtocolError::params(format!("{PROTOCOL_VERSION} must be a string")))?;
        if version == STATELESS_REVISION {
            if meta.take(CLIENT_CAPABILITIES).is_none() {
                return Err(ProtocolError::params(format!(
                    "_meta is missing {CLIENT_CAPABILITIES}"
                )));
            }
            return Ok(Era::Stateless);
        }
        if HANDSHAKE_REVISIONS.contains(&version.as_str()) {
            return Ok(Era::Handshake);
        }
        let mut supported = vec![STATELESS_REVISION];
        supported.extend(HANDSHAKE_REVISIONS.into_iter().rev());
        Err(ProtocolError {
            code: UNSUPPORTED_PROTOCOL_VERSION,
            message: "Unsupported protocol version".into(),
            data: Some(json!({"supported": supported, "requested": version})),
        })
    }

    /// Adds what every result of the stateless revision carries.
    fn finish(&self, era: Era, mut result: Json) -> Json {
        if era == Era::Stateless {
            result["resultType"] = json!("complete");
            result["_meta"] = json!({SERVER_INFO: server_info()});
        }
        result
    }

    fn any_writable(&self) -> bool {
        self.stores.iter().any(|store| store.writable)
    }

    /// Runs one tool. The text is the tool's result.
    fn call(&mut self, tool: Tool, mut arguments: Members) -> Result<String, Failure> {
        let store = self.store(tool, arguments.take("store"))?;
        let handle = self.stores[store].handle.clone();
        if arguments.contains("handle") {
            return Err(Failure::InvalidParams("unknown member: handle".into()));
        }
        // Page arguments belong to this front end, not to the dispatch.
        let (offset, limit) = if tool.items().is_some() {
            let number = |arguments: &mut Members, key: &str| -> Result<Option<u64>, Failure> {
                arguments
                    .take(key)
                    .map(|raw| {
                        json::unsigned(&raw).ok_or_else(|| {
                            Failure::InvalidParams(format!("{key} must be an unsigned integer"))
                        })
                    })
                    .transpose()
            };
            let offset = number(&mut arguments, "offset")?.unwrap_or(0);
            let limit = number(&mut arguments, "limit")?;
            if limit == Some(0) {
                return Err(Failure::invalid("limit", "limit must be at least 1"));
            }
            if offset > 0 && !arguments.contains("at") {
                return Err(Failure::invalid(
                    "offset",
                    "a page after the first needs at: pass the basis of the first page",
                ));
            }
            (offset, limit)
        } else {
            (0, None)
        };
        if tool == Tool::Transact
            && let Some(request) = arguments.take("tx")
        {
            let stamped = stamp(&request, &self.options.metadata)?;
            arguments.insert(
                "tx",
                RawValue::from_string(stamped).expect("a stamped request is JSON"),
            );
        }
        arguments.insert(
            "handle",
            RawValue::from_string(json::quote(&handle)).expect("a quoted string is JSON"),
        );
        match self.dispatcher.dispatch(tool.name(), arguments)? {
            Reply::Value(snapshot) if tool == Tool::Head => Ok(format!(
                r#"{{"now":{{"inst":"{}"}},"snapshot":{snapshot}}}"#,
                now_millis()
            )),
            Reply::Value(value) => Ok(value),
            Reply::Read { basis, value } => {
                let basis = factlite::wire::encode_snapshot(&basis);
                let Some(items) = tool.items() else {
                    return Ok(format!(r#"{{"basis":{basis},"value":{value}}}"#));
                };
                let mut value: Json = serde_json::from_str(&value).expect("wire output is JSON");
                let page = self.cut(
                    value[items].as_array_mut().expect("a paged read has items"),
                    offset,
                    limit,
                );
                Ok(format!(
                    r#"{{"basis":{basis},"page":{page},"value":{value}}}"#
                ))
            }
        }
    }

    /// Keeps one page of `items` and describes it.
    fn cut(&self, items: &mut Vec<Json>, offset: u64, limit: Option<u64>) -> Json {
        let total = items.len();
        let offset = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
        let most = limit
            .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX))
            .unwrap_or(usize::MAX)
            .min(self.options.page_items);
        let mut count = 0;
        let mut bytes = 0usize;
        for item in &items[offset..] {
            let size = item.to_string().len();
            if count == most || (count > 0 && bytes + size > self.options.page_bytes) {
                break;
            }
            bytes += size;
            count += 1;
        }
        items.truncate(offset + count);
        items.drain(..offset);
        let mut page = json!({"offset": offset, "count": count, "total": total});
        if offset + count < total {
            page["next"] = json!(offset + count);
        }
        page
    }

    /// Resolves the `store` argument to an index into the bound stores.
    fn store(&self, tool: Tool, argument: Option<Box<RawValue>>) -> Result<usize, Failure> {
        let names = || {
            self.stores
                .iter()
                .map(|store| store.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let index = match argument {
            None if self.stores.len() == 1 => 0,
            None => {
                return Err(Failure::invalid(
                    "store",
                    format!("name a store: {}", names()),
                ));
            }
            Some(raw) => {
                let name = json::string(&raw)
                    .ok_or_else(|| Failure::InvalidParams("store must be a string".into()))?;
                self.stores
                    .iter()
                    .position(|store| store.name == name)
                    .ok_or_else(|| {
                        Failure::invalid(
                            "store",
                            format!("unknown store: {name}; bound stores: {}", names()),
                        )
                    })?
            }
        };
        if tool == Tool::Transact && !self.stores[index].writable {
            return Err(Failure::invalid(
                "store",
                format!("store {} is bound read-only", self.stores[index].name),
            ));
        }
        Ok(index)
    }

    fn tools(&self) -> Vec<Json> {
        let names = |writable_only: bool| -> Vec<&str> {
            self.stores
                .iter()
                .filter(|store| store.writable || !writable_only)
                .map(|store| store.name.as_str())
                .collect()
        };
        Tool::ALL
            .into_iter()
            .filter(|tool| *tool != Tool::Transact || self.any_writable())
            .map(|tool| {
                let writes = tool == Tool::Transact;
                let (description, mut properties, mut required) = definition(tool);
                let stores = names(writes);
                properties["store"] = json!({
                    "type": "string",
                    "enum": stores,
                    "description": "Which store. May be omitted when one store is bound.",
                });
                if self.stores.len() > 1 {
                    required.push("store");
                }
                if tool.items().is_some() {
                    properties["offset"] = json!({
                        "type": "integer",
                        "minimum": 0,
                        "description": "Skip this many items. Needs at. Use page.next from the previous page.",
                    });
                    properties["limit"] = json!({
                        "type": "integer",
                        "minimum": 1,
                        "description": format!(
                            "Return at most this many items. The server's page holds at most {}.",
                            self.options.page_items
                        ),
                    });
                }
                json!({
                    "name": tool.name(),
                    "description": description,
                    "inputSchema": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                        "additionalProperties": false,
                    },
                    "annotations": {
                        "readOnlyHint": !writes,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false,
                    },
                })
            })
            .collect()
    }

    fn instructions(&self) -> String {
        let stores = self
            .stores
            .iter()
            .map(|store| {
                format!(
                    "{} ({})",
                    store.name,
                    if store.writable {
                        "writable"
                    } else {
                        "read-only"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut text = format!(
            "Factlite stores typed facts (entity, attribute, value) with full history. Stores: {stores}.\n\
             - Values are tagged with their type: {{\"string\":\"x\"}}, {{\"i64\":\"3\"}}, {{\"f64\":\"1.5\"}}, {{\"bool\":true}}, {{\"keyword\":\"ns/name\"}}, {{\"inst\":\"<milliseconds since 1970>\"}}, {{\"bytes\":\"<base64>\"}}, {{\"ref\":{{\"id\":\"e1...\"}}}}. Integers are decimal strings.\n\
             - Call schema first: attributes must be defined before use and never change.\n\
             - Every read returns basis, the snapshot it used. Pass it as at to later reads to see the same state while others write. Without at a read uses the latest state.\n\
             - Long results come in pages. When page.next is present, repeat the call with the same arguments, at set to basis and offset set to page.next.\n\
             - Nothing is overwritten: history lists every past value, and a read at an old basis returns the old state.\n"
        );
        if self.any_writable() {
            text.push_str(
                "- transact needs idempotency_key, a string naming this one intended change. If a call may not have gone through, send exactly the same request again: the first report comes back and nothing is committed twice. A different request under a used key is refused.\n\
                 - To change a value others may also change, add expect with the values you read; the commit is refused with PreconditionFailed if they changed. Then read again and decide.\n\
                 - Unique values do not upsert: look an entity up, create it if absent, and on UniqueViolation look it up again.\n\
                 - Facts are permanent. Nothing written can be deleted, and a retracted value stays in history. Never write secrets, credentials, raw tool or file output, conversation text, or personal data. Write a reference and a short summary instead.\n\
                 - head returns now, the current time as an inst, for timestamps you write. Take it once per step so a retry is identical.\n",
            );
        }
        text
    }
}

fn server_info() -> Json {
    json!({"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")})
}

fn schema_uri(store: &str) -> String {
    format!("factlite://{store}/schema")
}

fn now_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Marks a result of the stateless revision as reusable by its client for
/// `ttl_ms`. Every result here describes one person's configuration or data.
fn cached(mut result: Json, ttl_ms: u64) -> Json {
    result["ttlMs"] = json!(ttl_ms);
    result["cacheScope"] = json!("private");
    result
}

/// The Factlite error object for any failure of a tool call.
fn failure_json(failure: &Failure) -> Json {
    match failure.data() {
        Some(data) => serde_json::from_str(&data).expect("failure data is JSON"),
        None => argument_error(&failure.message()),
    }
}
fn argument_error(message: &str) -> Json {
    json!({"code": "InvalidRequest", "message": message, "context": {"field": "arguments"}})
}
fn tool_error(error: &Json) -> Json {
    json!({
        "content": [{"type": "text", "text": json!({"error": error}).to_string()}],
        "isError": true,
    })
}

/// Adds the server's metadata to a transaction request, as text. A request
/// that is not an object with an object for `metadata` is returned as it
/// came, for the wire decoder to refuse in its own words.
fn stamp(request: &RawValue, metadata: &[(String, String)]) -> Result<String, Failure> {
    let original = || Ok(request.get().to_owned());
    if metadata.is_empty() {
        return original();
    }
    let Ok(Top::Object(mut members)) = Top::parse(request.get()) else {
        return original();
    };
    let mut entries: Vec<(String, String)> = match members.take("metadata") {
        None => Vec::new(),
        Some(raw) => match Top::parse(raw.get()) {
            Ok(Top::Object(given)) => given
                .into_vec()
                .into_iter()
                .map(|(key, value)| (key, value.get().to_owned()))
                .collect(),
            _ => return original(),
        },
    };
    for (key, value) in metadata {
        if entries.iter().any(|(given, _)| given == key) {
            return Err(Failure::invalid(
                "metadata",
                format!("metadata key {key} is set by the server and cannot be sent"),
            ));
        }
        entries.push((key.clone(), json!({"string": value}).to_string()));
    }
    let object = |entries: Vec<(String, String)>| {
        let members: Vec<String> = entries
            .into_iter()
            .map(|(key, value)| format!("{}:{value}", json::quote(&key)))
            .collect();
        format!("{{{}}}", members.join(","))
    };
    let mut stamped: Vec<(String, String)> = members
        .into_vec()
        .into_iter()
        .map(|(key, value)| (key, value.get().to_owned()))
        .collect();
    stamped.push(("metadata".into(), object(entries)));
    Ok(object(stamped))
}

/// A tool's description, the properties of its arguments, and which are
/// required. `store`, `offset` and `limit` are added by the caller.
fn definition(tool: Tool) -> (&'static str, Json, Vec<&'static str>) {
    let at = json!({
        "type": ["object", "string"],
        "description": "Snapshot to read at: a basis or snapshot from an earlier result. Omit for the latest state.",
    });
    let value = json!({
        "type": "object",
        "minProperties": 1,
        "maxProperties": 1,
        "description": "One tagged value, such as {\"string\":\"x\"} or {\"i64\":\"3\"}.",
    });
    let entity = json!({
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"],
        "additionalProperties": false,
        "description": "An existing entity: {\"id\":\"e1...\"}.",
    });
    let attr = json!({"type": "string", "description": "Attribute name, namespace/name."});
    let filter = json!({
        "type": "object",
        "properties": {"entity": entity, "attr": attr, "value": value},
        "additionalProperties": false,
        "description": "Restrict to an entity, an attribute, a value, or any combination.",
    });
    let after = json!({
        "type": "string",
        "pattern": "^(0|[1-9][0-9]*)$",
        "description": "A revision as a decimal string. Only what happened after it is returned. Default \"0\".",
    });
    match tool {
        Tool::Head => (
            "The latest snapshot of a store, and now, the current time as an inst value.",
            json!({}),
            vec![],
        ),
        Tool::Schema => (
            "The attributes defined in a store: name, value type, cardinality (one or many) and uniqueness.",
            json!({"at": at}),
            vec![],
        ),
        Tool::Facts => (
            "Current facts, optionally restricted by entity, attribute and value. Ordered by entity, attribute, value.",
            json!({"filter": filter, "at": at}),
            vec![],
        ),
        Tool::Pull => (
            "The named attributes of one entity. A one-valued attribute gives a value, a many-valued one an array, an absent one is omitted. An unknown entity gives entity null.",
            json!({
                "entity": entity,
                "attrs": {"type": "array", "items": {"type": "string"}, "description": "Attribute names to return."},
                "at": at,
            }),
            vec!["entity", "attrs"],
        ),
        Tool::Lookup => (
            "The entity that holds a value of a unique attribute, or null.",
            json!({"attr": attr, "value": value, "at": at}),
            vec!["attr", "value"],
        ),
        Tool::History => (
            "Every assertion and retraction after a revision, oldest first, optionally restricted like facts. Each event has its revision.",
            json!({"filter": filter, "after": after, "at": at}),
            vec![],
        ),
        Tool::Transactions => (
            "Every commit after a revision, oldest first, with its metadata and what it changed. Use it to see what changed since a basis and who wrote it.",
            json!({"after": after, "at": at}),
            vec![],
        ),
        Tool::Query => (
            "A conjunctive query over facts. match is a list of [entity, \"attribute\", value] patterns whose positions are {\"var\":\"name\"}, {\"param\":\"name\"} or a tagged value; shared variables join. select names the variables to return. Give order a total order when paging.",
            json!({
                "query": {
                    "type": "object",
                    "properties": {
                        "version": {"const": 1},
                        "query": {
                            "type": "object",
                            "properties": {
                                "inputs": {"type": "object", "description": "Declared parameters: name to value type."},
                                "match": {"type": "array", "items": {"type": "array", "minItems": 3, "maxItems": 3}},
                                "where": {
                                    "type": "array",
                                    "description": "Comparisons: {\"left\":term,\"op\":\"eq|ne|lt|le|gt|ge\",\"right\":term}.",
                                    "items": {"type": "object"},
                                },
                                "not": {
                                    "type": "array",
                                    "description": "Negated groups: {\"match\":[...],\"where\":[...],\"join\":[names]?}.",
                                    "items": {"type": "object"},
                                },
                                "select": {"type": "array", "items": {"type": "string"}},
                                "order": {
                                    "type": "array",
                                    "description": "{\"field\":\"selected name\",\"direction\":\"asc|desc\"}.",
                                    "items": {"type": "object"},
                                },
                                "limit": {"type": "integer", "minimum": 0},
                                "with": {"type": "array", "items": {"type": "string"}},
                                "aggregates": {
                                    "type": "array",
                                    "description": "{\"fn\":\"count|count_distinct|sum|avg|min|max\",\"var\":name,\"name\":column}.",
                                    "items": {"type": "object"},
                                },
                                "having": {"type": "array", "items": {"type": "object"}},
                            },
                            "required": ["match", "select"],
                            "additionalProperties": false,
                        },
                        "params": {"type": "object", "description": "A tagged value for each declared input."},
                    },
                    "required": ["version", "query"],
                    "additionalProperties": false,
                },
                "at": at,
            }),
            vec!["query"],
        ),
        Tool::Transact => (
            "Commit one atomic change: define attributes, create entities, assert and retract facts. Returns a report with the snapshots before and after and the ids of created entities. Asserting a one-valued attribute replaces its value. Requires idempotency_key.",
            json!({
                "tx": {
                    "type": "object",
                    "properties": {
                        "version": {"const": 1},
                        "idempotency_key": {"type": "string", "description": "Names this one intended change. Resend unchanged to retry."},
                        "schema": {
                            "type": "array",
                            "description": "New attributes.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string"},
                                    "type": {"enum": ["bool", "i64", "f64", "string", "bytes", "ref", "keyword", "inst"]},
                                    "cardinality": {"enum": ["one", "many"]},
                                    "unique": {"enum": ["none", "unique_one"]},
                                },
                                "required": ["name", "type", "cardinality", "unique"],
                                "additionalProperties": false,
                            },
                        },
                        "new_entities": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Labels for entities this request creates. Use {\"temp\":\"label\"} for them below.",
                        },
                        "changes": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "op": {"enum": ["assert", "retract"]},
                                    "entity": {
                                        "type": "object",
                                        "description": "{\"id\":\"e1...\"}, {\"temp\":\"label\"}, or {\"lookup\":{\"attr\":unique attribute,\"value\":tagged value}}.",
                                    },
                                    "attr": {"type": "string"},
                                    "value": {
                                        "type": "object",
                                        "description": "A tagged value. A reference may be {\"ref\":{\"temp\":\"label\"}} or {\"ref\":{\"lookup\":{...}}}.",
                                    },
                                },
                                "required": ["op", "entity", "attr", "value"],
                                "additionalProperties": false,
                            },
                        },
                        "expect": {
                            "type": "array",
                            "description": "Refuse the commit unless each attribute of each entity holds exactly these values. [] expects none.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "entity": entity,
                                    "attr": {"type": "string"},
                                    "values": {"type": "array", "items": {"type": "object"}},
                                },
                                "required": ["entity", "attr", "values"],
                                "additionalProperties": false,
                            },
                        },
                        "metadata": {"type": "object", "description": "Notes about this commit: name to tagged value."},
                        "expected_head": {"type": "object", "description": "Refuse the commit unless this snapshot is still the latest."},
                    },
                    "required": ["version", "idempotency_key"],
                    "additionalProperties": false,
                },
            }),
            vec!["tx"],
        ),
    }
}
