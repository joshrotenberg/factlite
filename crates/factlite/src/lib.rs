//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persists atomic reports and retained
//! semantic history with explicit commit outcome evidence.
//!
//! A new no-change transaction still advances the revision. A keyed exact retry
//! returns its original report, even after later commits:
//!
//! ```
//! use factlite::{Store, Tx};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let directory = tempfile::tempdir()?;
//! let store = Store::open(directory.path().join("example.sqlite"))?;
//! let request = Tx::builder().idempotency_key("first-operation").build()?;
//! let report = store.commit(request.clone())?;
//! let before = store.db_at(&report.after)?;
//! let later = store.commit(Tx::default())?;
//! assert_eq!(before.snapshot(), &report.after);
//! assert_eq!(store.commit(request)?, report);
//! assert_eq!(store.head()?, later.after);
//! # Ok(())
//! # }
//! ```
//!
//! Run `cargo run --locked -p factlite --example task_workflow` from the checkout
//! for a complete graph, conditional update, query, backup and reopen workflow.

mod backup;
mod canonical;
mod canonical_decode;
mod catalog;
mod codec;
mod db;
mod error;
#[cfg(test)]
mod faults;
mod identity;
#[cfg(test)]
mod isolation;
mod log;
mod model;
#[cfg(test)]
mod process_recovery;
mod query;
mod query_eval;
mod query_sql;
#[cfg(test)]
mod replay_checks;
#[cfg(test)]
mod scenarios;
mod schema;
mod store;
mod tx;
mod value;
pub mod wire;

pub use canonical::CanonicalRequest;
pub use db::{Db, Entity, FactEvent, FactFilter, PulledValue, TxRecord};
pub use error::{CommitOutcome, Error, ErrorCode, ErrorContext, Result};
pub use identity::{EntityId, LineageId, Revision, SnapshotId, StoreId, TempId};
pub use model::{Fact, FactDelta, TxReport};
pub use query::{
    Comparison, Direction, Order, Params, Pattern, Predicate, Query, QueryName, QueryResult, Term,
};
pub use schema::{AttrName, AttributeDefinition, Cardinality, Schema, Uniqueness};
pub use store::Store;
pub use tx::{Change, EntityTerm, Expectation, Operation, Tx, TxBuilder, ValueTerm};
pub use value::{FiniteF64, Value, ValueType};
