//! A babashka pod for Factlite: typed facts and durable revision-pinned
//! database views over SQLite.
//!
//! The pod is a thin adapter. Requests are translated from Clojure data into
//! version-1 wire JSON and decoded by the library's strict wire decoders;
//! storage semantics stay in the library. See `crates/factlite-pod/README.md`
//! for the client API and the value mapping.
//!
//! The namespace needs [`Format::TransitJson`](babashka_pod::Format): keywords,
//! symbols, byte arrays and the long/double distinction only survive in that
//! format.

mod convert;

use babashka_pod::transit::Value as Clj;
use babashka_pod::{Handle, Handles, Namespace, PodError};
use convert::{Fields, Result, invalid, library_error};
use factlite::{Db, Revision, SnapshotId, Store};

/// The namespace this pod defines.
pub const NAMESPACE: &str = "pod.joshrotenberg.factlite";

const KIND: &str = "factlite.store";

fn arity(args: &[Clj], var: &str, min: usize, max: usize, usage: &str) -> Result<()> {
    if args.len() < min || args.len() > max {
        return Err(invalid(
            "arguments",
            format!("{NAMESPACE}/{var} expects {usage}"),
        ));
    }
    Ok(())
}

fn handle_in(value: &Clj) -> Result<Handle> {
    let mut fields = Fields::new(value, "connection")?;
    let handle = convert::text(
        &fields
            .take("handle")
            .ok_or_else(|| invalid("connection", "not a factlite connection"))?,
        "handle",
    )?;
    let id = match fields.take("id") {
        Some(Clj::Int(id)) if id >= 0 => id as u64,
        _ => return Err(invalid("connection", "not a factlite connection")),
    };
    fields.finish()?;
    Ok(Handle { handle, id })
}

fn handle_out(handle: &Handle) -> Clj {
    Clj::Map(vec![
        (convert::kw("handle"), Clj::String(handle.handle.clone())),
        (convert::kw("id"), Clj::Int(handle.id as i64)),
    ])
}

/// Clones the store out of the registry, so its lock is held only briefly.
/// `Store` serializes its own connection use.
fn store(stores: &Handles<Store>, value: &Clj) -> Result<Store> {
    stores.with(&handle_in(value)?, |store| Ok(store.clone()))
}

/// The view for a read: head, a full snapshot map, or a revision number
/// resolved against this store's identity and lineage. Every form is
/// validated by `Store::db_at`; a future revision fails `InvalidSnapshot`.
fn view(store: &Store, at: Option<Clj>) -> Result<Db> {
    let snapshot = match at {
        None | Some(Clj::Nil) => return store.db().map_err(library_error),
        Some(snapshot @ Clj::Map(_)) => convert::snapshot_in(&snapshot)?,
        Some(revision) => {
            let head = store.head().map_err(library_error)?;
            let revision: Revision = convert::revision_in(&revision, "at")?;
            SnapshotId::new(head.store().clone(), head.lineage().clone(), revision)
        }
    };
    store.db_at(&snapshot).map_err(library_error)
}

fn after(fields: &mut Fields) -> Result<Revision> {
    fields.take("after").map_or(Ok(Revision::ZERO), |after| {
        convert::revision_in(&after, "after")
    })
}

type Var = Box<dyn Fn(Vec<Clj>) -> std::result::Result<Clj, PodError> + Send + Sync>;

fn vars(
    stores: &Handles<Store>,
) -> Vec<(&'static str, &'static str, &'static [&'static str], Var)> {
    let s = stores.clone();
    let open: Var = Box::new(move |args| {
        arity(&args, "open", 1, 1, "[path]")?;
        let path = convert::text(&args[0], "path")?;
        let store = Store::open(&path).map_err(library_error)?;
        Ok(handle_out(&s.insert(store)))
    });
    let s = stores.clone();
    let close: Var = Box::new(move |args| {
        arity(&args, "close", 1, 1, "[conn]")?;
        s.remove(&handle_in(&args[0])?)?;
        Ok(Clj::Nil)
    });
    let s = stores.clone();
    let head: Var = Box::new(move |args| {
        arity(&args, "head", 1, 1, "[conn]")?;
        let head = store(&s, &args[0])?.head().map_err(library_error)?;
        Ok(convert::snapshot_out(&head))
    });
    let s = stores.clone();
    let transact: Var = Box::new(move |args| {
        arity(&args, "transact", 2, 2, "[conn tx]")?;
        let store = store(&s, &args[0])?;
        let tx = convert::tx_in(&args[1])?;
        let report = store.commit(tx).map_err(library_error)?;
        Ok(convert::report_out(&report))
    });
    let s = stores.clone();
    let schema: Var = Box::new(move |args| {
        arity(&args, "schema", 1, 2, "[conn] or [conn opts]")?;
        let store = store(&s, &args[0])?;
        let mut opts = Fields::optional(args.get(1), "opts")?;
        let db = view(&store, opts.take("at"))?;
        opts.finish()?;
        let schema = db.schema().map_err(library_error)?;
        Ok(Clj::Vector(
            schema.attributes().map(convert::definition_out).collect(),
        ))
    });
    let s = stores.clone();
    let pull: Var = Box::new(move |args| {
        arity(
            &args,
            "pull",
            3,
            4,
            "[conn entity attrs] or [conn entity attrs opts]",
        )?;
        let store = store(&s, &args[0])?;
        let id = convert::entity_id(&args[1])?;
        let attrs = match &args[2] {
            Clj::Vector(attrs) | Clj::List(attrs) | Clj::Set(attrs) => attrs
                .iter()
                .map(convert::attr)
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(invalid(
                    "attrs",
                    "attrs must be a vector of attribute names",
                ));
            }
        };
        let mut opts = Fields::optional(args.get(3), "opts")?;
        let db = view(&store, opts.take("at"))?;
        opts.finish()?;
        let entity = db.pull(&id, &attrs).map_err(library_error)?;
        Ok(convert::entity_out(entity.as_ref()))
    });
    let s = stores.clone();
    let facts: Var = Box::new(move |args| {
        arity(
            &args,
            "facts",
            1,
            3,
            "[conn], [conn filter] or [conn filter opts]",
        )?;
        let store = store(&s, &args[0])?;
        let filter = convert::filter_in(args.get(1))?;
        let mut opts = Fields::optional(args.get(2), "opts")?;
        let db = view(&store, opts.take("at"))?;
        opts.finish()?;
        let facts = db.facts(filter).map_err(library_error)?;
        Ok(Clj::Vector(facts.iter().map(convert::fact_out).collect()))
    });
    let s = stores.clone();
    let history: Var = Box::new(move |args| {
        arity(
            &args,
            "history",
            1,
            3,
            "[conn], [conn filter] or [conn filter opts]",
        )?;
        let store = store(&s, &args[0])?;
        let filter = convert::filter_in(args.get(1))?;
        let mut opts = Fields::optional(args.get(2), "opts")?;
        let db = view(&store, opts.take("at"))?;
        let after = after(&mut opts)?;
        opts.finish()?;
        let events = db.history(filter, after).map_err(library_error)?;
        Ok(Clj::Vector(events.iter().map(convert::event_out).collect()))
    });
    let s = stores.clone();
    let transactions: Var = Box::new(move |args| {
        arity(&args, "transactions", 1, 2, "[conn] or [conn opts]")?;
        let store = store(&s, &args[0])?;
        let mut opts = Fields::optional(args.get(1), "opts")?;
        let db = view(&store, opts.take("at"))?;
        let after = after(&mut opts)?;
        opts.finish()?;
        let records = db.transactions(after).map_err(library_error)?;
        Ok(Clj::Vector(
            records
                .iter()
                .map(|record| convert::report_out(&record.report))
                .collect(),
        ))
    });
    let s = stores.clone();
    let q: Var = Box::new(move |args| {
        arity(
            &args,
            "q",
            2,
            4,
            "[conn query], [conn query params] or [conn query params opts]",
        )?;
        let store = store(&s, &args[0])?;
        let (query, params) = convert::query_in(&args[1], args.get(2))?;
        let mut opts = Fields::optional(args.get(3), "opts")?;
        let db = view(&store, opts.take("at"))?;
        opts.finish()?;
        let result = db.query(&query, &params).map_err(library_error)?;
        Ok(convert::result_out(&result))
    });
    vec![
        (
            "open",
            "Opens or creates a Factlite database file and returns a connection.",
            &["[path]"],
            open,
        ),
        (
            "close",
            "Closes a connection. Later calls with it fail.",
            &["[conn]"],
            close,
        ),
        (
            "head",
            "Returns the latest committed snapshot.",
            &["[conn]"],
            head,
        ),
        (
            "transact",
            "Commits one declarative transaction and returns its report. \
             Failures are ex-info with :code, :message, :context and :commit-outcome.",
            &["[conn tx]"],
            transact,
        ),
        (
            "schema",
            "Returns attribute definitions visible at head or opts :at.",
            &["[conn]", "[conn opts]"],
            schema,
        ),
        (
            "pull",
            "Returns {:id ...} plus the present requested attributes, or nil for an \
             entity unknown at head or opts :at.",
            &["[conn entity attrs]", "[conn entity attrs opts]"],
            pull,
        ),
        (
            "facts",
            "Returns [entity attr value] facts matching an optional filter map.",
            &["[conn]", "[conn filter]", "[conn filter opts]"],
            facts,
        ),
        (
            "history",
            "Returns effective fact events in (opts :after, view revision].",
            &["[conn]", "[conn filter]", "[conn filter opts]"],
            history,
        ),
        (
            "transactions",
            "Returns committed reports in (opts :after, view revision], including no-change commits.",
            &["[conn]", "[conn opts]"],
            transactions,
        ),
        (
            "q",
            "Runs a conjunctive query map with optional params; returns {:columns :rows}.",
            &[
                "[conn query]",
                "[conn query params]",
                "[conn query params opts]",
            ],
            q,
        ),
    ]
}

/// Builds the namespace with a fresh connection registry. Connections are
/// closed when `close` is called or when the pod shuts down.
pub fn namespace() -> Namespace {
    let stores: Handles<Store> = Handles::new(KIND);
    let mut namespace = Namespace::new(NAMESPACE);
    for (name, doc, arglists, var) in vars(&stores) {
        namespace = namespace
            .function_transit(name, var)
            .doc(name, doc)
            .arglists(name, arglists.iter().copied());
    }
    namespace
}
