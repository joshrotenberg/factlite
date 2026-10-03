//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persists atomic reports and retained
//! semantic history with explicit commit outcome evidence.

mod catalog;
mod codec;
mod error;
#[cfg(test)]
mod faults;
mod identity;
mod log;
mod model;
#[cfg(test)]
mod scenarios;
mod schema;
mod store;
mod tx;
mod value;

pub use error::{CommitOutcome, Error, ErrorCode, ErrorContext, Result};
pub use identity::{EntityId, LineageId, Revision, SnapshotId, StoreId, TempId};
pub use model::{Fact, FactDelta, TxReport};
pub use schema::{AttrName, AttributeDefinition, Cardinality, Schema, Uniqueness};
pub use store::Store;
pub use tx::{Change, EntityTerm, Expectation, Operation, Tx, TxBuilder, ValueTerm};
pub use value::{FiniteF64, Value, ValueType};
