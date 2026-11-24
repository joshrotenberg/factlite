//! Method dispatch: a method name and its params in, a JSON result or a
//! classified failure out. No storage semantics live here.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use factlite::{Db, Revision, SnapshotId, Store, StoreOptions, Validation, wire};
use serde_json::value::RawValue;

use crate::json::{self, Malformed, Members, Top};

/// Settings that change what the dispatcher accepts.
#[derive(Clone, Debug, Default)]
pub struct DispatchOptions {
    /// Refuse a `transact` whose request carries no idempotency key.
    pub require_idempotency_key: bool,
}

/// Why a call produced no result.
#[derive(Debug)]
pub enum Failure {
    /// The method does not exist. JSON-RPC code -32601.
    MethodNotFound(String),
    /// The params are missing, not an object, or have a missing, duplicate,
    /// unknown or wrongly typed member that this interface defines. JSON-RPC
    /// code -32602.
    InvalidParams(String),
    /// The library refused or failed the request.
    Library(factlite::Error),
    /// A failure of the same shape as a library error that this adapter
    /// raises itself: an unknown or read-only handle, or a path it would not
    /// create.
    Adapter {
        /// The Factlite error code, `InvalidRequest` or `Storage`.
        code: &'static str,
        /// A human-readable message.
        message: String,
        /// The error context, as JSON object text.
        context: String,
    },
}

impl Failure {
    fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Adapter {
            code: "InvalidRequest",
            message: message.into(),
            context: format!(r#"{{"field":{}}}"#, json::quote(field)),
        }
    }
    fn io(operation: &str, error: &std::io::Error) -> Self {
        Self::Adapter {
            code: "Storage",
            message: format!("{operation}: {error}"),
            context: format!(r#"{{"operation":{}}}"#, json::quote(operation)),
        }
    }

    /// The JSON-RPC error code.
    pub fn code(&self) -> i64 {
        match self {
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::Library(_) | Self::Adapter { .. } => FACTLITE_ERROR,
        }
    }
    /// The JSON-RPC error message.
    pub fn message(&self) -> String {
        match self {
            Self::MethodNotFound(method) => format!("method not found: {method}"),
            Self::InvalidParams(message) => format!("invalid params: {message}"),
            Self::Library(error) => error.to_string(),
            Self::Adapter { message, .. } => message.clone(),
        }
    }
    /// The Factlite error object `{code, message, context, commit_outcome?}`
    /// as JSON text, for Factlite failures. Protocol failures have none.
    pub fn data(&self) -> Option<String> {
        match self {
            Self::MethodNotFound(_) | Self::InvalidParams(_) => None,
            Self::Library(error) => {
                // encode_error wraps the object as {"version":1,"error":{...}}.
                let mut wrapped = match Top::parse(&wire::encode_error(error)) {
                    Ok(Top::Object(members)) => members,
                    _ => unreachable!("encode_error returns an object"),
                };
                let inner = wrapped.take("error").expect("encode_error has an error");
                Some(inner.get().to_owned())
            }
            Self::Adapter {
                code,
                message,
                context,
            } => Some(format!(
                r#"{{"code":{},"context":{context},"message":{}}}"#,
                json::quote(code),
                json::quote(message)
            )),
        }
    }
}
impl From<factlite::Error> for Failure {
    fn from(error: factlite::Error) -> Self {
        Self::Library(error)
    }
}

/// The JSON-RPC code of every Factlite error. Clients branch on
/// `error.data.code`.
pub const FACTLITE_ERROR: i64 = -32000;

type Result<T> = std::result::Result<T, Failure>;

/// What a call returned.
#[derive(Debug)]
pub(crate) enum Reply {
    /// A complete JSON result.
    Value(String),
    /// A read: the snapshot it used and the wire output for that read.
    Read { basis: SnapshotId, value: String },
}
impl Reply {
    pub(crate) fn into_json(self) -> String {
        match self {
            Self::Value(value) => value,
            Self::Read { basis, value } => format!(
                r#"{{"basis":{},"value":{value}}}"#,
                wire::encode_snapshot(&basis)
            ),
        }
    }
}

/// One open connection: a writable store, or a read-only view that moves to
/// the latest head on each read that names no snapshot.
enum Conn {
    Writer(Store),
    Reader(Db),
}
impl Conn {
    fn view(&self, at: Option<&SnapshotId>) -> factlite::Result<Db> {
        match (self, at) {
            (Self::Writer(store), None) => store.db(),
            (Self::Writer(store), Some(snapshot)) => store.db_at(snapshot),
            (Self::Reader(db), None) => db.at_head(),
            (Self::Reader(db), Some(snapshot)) => db.at(snapshot),
        }
    }
    fn writer(&self) -> Result<&Store> {
        match self {
            Self::Writer(store) => Ok(store),
            Self::Reader(_) => Err(Failure::invalid("handle", "the handle is read-only")),
        }
    }
}

/// The params of one call, taken apart member by member.
struct Params(Members);
impl Params {
    fn required(&mut self, key: &str) -> Result<Box<RawValue>> {
        self.0
            .take(key)
            .ok_or_else(|| Failure::InvalidParams(format!("missing member: {key}")))
    }
    fn optional(&mut self, key: &str) -> Option<Box<RawValue>> {
        self.0.take(key)
    }
    fn string(&mut self, key: &str) -> Result<String> {
        let raw = self.required(key)?;
        json::string(&raw).ok_or_else(|| Failure::InvalidParams(format!("{key} must be a string")))
    }
    fn flag(&mut self, key: &str) -> Result<bool> {
        match self.optional(key) {
            None => Ok(false),
            Some(raw) => json::boolean(&raw)
                .ok_or_else(|| Failure::InvalidParams(format!("{key} must be a boolean"))),
        }
    }
    /// Every known member has been taken; anything left is unknown.
    fn finish(self) -> Result<()> {
        match self.0.remaining() {
            Some(key) => Err(Failure::InvalidParams(format!("unknown member: {key}"))),
            None => Ok(()),
        }
    }
}

/// A snapshot: the wire object, or the compact token as a JSON string.
fn snapshot(raw: &RawValue) -> factlite::Result<SnapshotId> {
    match json::string(raw) {
        Some(token) => token.parse(),
        None => wire::decode_snapshot(raw.get()),
    }
}

/// The methods whose effect depends on the order of requests. They are
/// refused inside a batch.
pub const ORDER_DEPENDENT: [&str; 5] = ["transact", "open", "close", "backup", "shutdown"];

/// Open connections and the methods that act on them.
#[derive(Default)]
pub struct Dispatcher {
    options: DispatchOptions,
    handles: BTreeMap<String, Conn>,
    opened: u64,
}

impl Dispatcher {
    /// A dispatcher with nothing open.
    pub fn new(options: DispatchOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    /// Runs one method. `params` is the raw text of the request's `params`
    /// member, or None when the request has none. The result is JSON text.
    pub fn call(
        &mut self,
        method: &str,
        params: Option<&str>,
    ) -> std::result::Result<String, Failure> {
        let members = match params {
            None => Members::default(),
            Some(text) => match Top::parse(text) {
                Ok(Top::Object(members)) => members,
                Ok(_) => return Err(Failure::InvalidParams("params must be an object".into())),
                Err(Malformed::Duplicate(message)) => return Err(Failure::InvalidParams(message)),
                Err(Malformed::Syntax(message)) => return Err(Failure::InvalidParams(message)),
            },
        };
        self.dispatch(method, members).map(Reply::into_json)
    }

    /// How many handles are open.
    pub fn open_handles(&self) -> usize {
        self.handles.len()
    }

    pub(crate) fn dispatch(&mut self, method: &str, members: Members) -> Result<Reply> {
        let mut params = Params(members);
        match method {
            "open" => self.open(params),
            "close" => {
                let handle = params.string("handle")?;
                params.finish()?;
                self.conn(&handle)?;
                self.handles.remove(&handle);
                Ok(Reply::Value("{}".into()))
            }
            "shutdown" => {
                params.finish()?;
                self.handles.clear();
                Ok(Reply::Value("{}".into()))
            }
            "head" => {
                let handle = params.string("handle")?;
                params.finish()?;
                let head = match self.conn(&handle)? {
                    Conn::Writer(store) => store.head()?,
                    Conn::Reader(db) => db.at_head()?.snapshot().clone(),
                };
                Ok(Reply::Value(wire::encode_snapshot(&head)))
            }
            "verify" => {
                let handle = params.string("handle")?;
                params.finish()?;
                let verified = match self.conn(&handle)? {
                    Conn::Writer(store) => store.verify()?,
                    Conn::Reader(db) => db.verify()?,
                };
                Ok(Reply::Value(wire::encode_snapshot(&verified)))
            }
            "transact" => {
                let handle = params.string("handle")?;
                let request = params.required("tx")?;
                params.finish()?;
                let store = self.conn(&handle)?.writer()?;
                let request = wire::decode_tx(request.get())?;
                if self.options.require_idempotency_key && request.idempotency_key().is_none() {
                    return Err(Failure::invalid(
                        "idempotency_key",
                        "this server requires an idempotency key on every transaction",
                    ));
                }
                Ok(Reply::Value(wire::encode_report(&store.commit(request)?)))
            }
            "backup" => {
                let handle = params.string("handle")?;
                let to = PathBuf::from(params.string("to")?);
                params.finish()?;
                let copy = self.conn(&handle)?.writer()?.backup(to)?;
                Ok(Reply::Value(wire::encode_snapshot(&copy)))
            }
            "schema" | "facts" | "pull" | "lookup" | "history" | "transactions" | "query" => {
                self.read(method, params)
            }
            _ => Err(Failure::MethodNotFound(method.to_owned())),
        }
    }

    fn conn(&self, handle: &str) -> Result<&Conn> {
        self.handles
            .get(handle)
            .ok_or_else(|| Failure::invalid("handle", format!("unknown handle: {handle}")))
    }

    fn open(&mut self, mut params: Params) -> Result<Reply> {
        let path = PathBuf::from(params.string("path")?);
        let create = params.flag("create")?;
        let read_only = params.flag("read_only")?;
        let validation = match params.optional("validation") {
            None => None,
            Some(raw) => Some(match json::string(&raw).as_deref() {
                Some("full") => Validation::Full,
                Some("structural") => Validation::Structural,
                _ => {
                    return Err(Failure::InvalidParams(
                        "validation must be \"full\" or \"structural\"".into(),
                    ));
                }
            }),
        };
        let timeout = match params.optional("busy_timeout_ms") {
            None => None,
            Some(raw) => Some(json::unsigned(&raw).ok_or_else(|| {
                Failure::InvalidParams("busy_timeout_ms must be an unsigned integer".into())
            })?),
        };
        params.finish()?;
        if create && read_only {
            return Err(Failure::invalid(
                "create",
                "a read-only open cannot create a store",
            ));
        }
        let mut options = StoreOptions::new();
        if let Some(millis) = timeout {
            options = options.with_busy_timeout(Duration::from_millis(millis))?;
        }
        if let Some(validation) = validation {
            options = options.with_validation(validation);
        }
        let (conn, head) = if read_only {
            let db = Db::open_read_only_with(&path, &options)?;
            let head = db.snapshot().clone();
            (Conn::Reader(db), head)
        } else {
            if !create {
                // Creation is explicit, though the library's open can initialize.
                std::fs::metadata(&path).map_err(|error| Failure::io("open database", &error))?;
            }
            let store = Store::open_with(&path, &options)?;
            let head = store.head()?;
            (Conn::Writer(store), head)
        };
        self.opened += 1;
        let handle = format!("h{}", self.opened);
        let reply = format!(
            r#"{{"handle":{},"head":{}}}"#,
            json::quote(&handle),
            wire::encode_snapshot(&head)
        );
        self.handles.insert(handle, conn);
        Ok(Reply::Value(reply))
    }

    fn read(&mut self, method: &str, mut params: Params) -> Result<Reply> {
        let handle = params.string("handle")?;
        let at = params.optional("at");
        // Take every member the method defines, so that a missing or unknown
        // member is reported before anything is decoded.
        let mut take = |key: &str, required: bool| -> Result<Option<Box<RawValue>>> {
            if required {
                params.required(key).map(Some)
            } else {
                Ok(params.optional(key))
            }
        };
        let (first, second) = match method {
            "schema" => (None, None),
            "facts" => (take("filter", false)?, None),
            "pull" => (take("entity", true)?, take("attrs", true)?),
            "lookup" => (take("attr", true)?, take("value", true)?),
            "history" => (take("filter", false)?, take("after", false)?),
            "transactions" => (take("after", false)?, None),
            "query" => (take("query", true)?, None),
            _ => unreachable!("read methods are matched by the caller"),
        };
        params.finish()?;
        let conn = self.conn(&handle)?;
        let at = at.as_deref().map(snapshot).transpose()?;
        let filter = |raw: Option<Box<RawValue>>| match raw {
            None => Ok(factlite::FactFilter::default()),
            Some(raw) => wire::decode_filter(raw.get()),
        };
        let after = |raw: Option<Box<RawValue>>| match raw {
            None => Ok(Revision::ZERO),
            Some(raw) => wire::decode_revision(raw.get()),
        };
        // Decode before opening the view, as the CLI decodes before it opens.
        enum Request {
            Schema,
            Facts(factlite::FactFilter),
            Pull(factlite::EntityId, Vec<factlite::AttrName>),
            Lookup(factlite::AttrName, factlite::Value),
            History(factlite::FactFilter, Revision),
            Transactions(Revision),
            Query(Box<(factlite::Query, factlite::Params)>),
        }
        let text =
            |raw: &Option<Box<RawValue>>| raw.as_ref().expect("required member").get().to_owned();
        let request = match method {
            "schema" => Request::Schema,
            "facts" => Request::Facts(filter(first)?),
            "pull" => Request::Pull(
                wire::decode_entity_id(&text(&first))?,
                wire::decode_attrs(&text(&second))?,
            ),
            "lookup" => Request::Lookup(
                wire::decode_attr(&text(&first))?,
                wire::decode_value(&text(&second))?,
            ),
            "history" => Request::History(filter(first)?, after(second)?),
            "transactions" => Request::Transactions(after(first)?),
            "query" => Request::Query(Box::new(wire::decode_query(&text(&first))?)),
            _ => unreachable!("read methods are matched by the caller"),
        };
        let db = conn.view(at.as_ref())?;
        let value = match request {
            Request::Schema => wire::encode_schema(&db.schema()?),
            Request::Facts(filter) => wire::encode_facts(&db.facts(filter)?),
            Request::Pull(entity, attrs) => wire::encode_entity(db.pull(&entity, &attrs)?.as_ref()),
            Request::Lookup(attribute, value) => {
                wire::encode_lookup(db.lookup(&attribute, &value)?.as_ref())
            }
            Request::History(filter, after) => wire::encode_history(&db.history(filter, after)?),
            Request::Transactions(after) => wire::encode_transactions(&db.transactions(after)?),
            Request::Query(request) => {
                let (query, params) = *request;
                wire::encode_query_result(&db.query(&query, &params)?)
            }
        };
        Ok(Reply::Read {
            basis: db.snapshot().clone(),
            value,
        })
    }
}
