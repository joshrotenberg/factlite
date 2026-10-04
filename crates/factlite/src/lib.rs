//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persists atomic reports and retained
//! semantic history with explicit commit outcome evidence.

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
mod query;
mod query_eval;
mod query_sql;
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
