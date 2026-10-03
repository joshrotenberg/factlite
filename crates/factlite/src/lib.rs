//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persistence is added separately.

mod catalog;
mod error;
mod identity;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Reference evaluator is tested before SQLite integration in step 2."
    )
)]
mod model;
#[cfg(test)]
mod scenarios;
mod schema;
mod store;
mod tx;
mod value;

pub use error::{Error, ErrorCode, ErrorContext, Result};
pub use identity::{EntityId, LineageId, Revision, SnapshotId, StoreId, TempId};
pub use model::{Fact, FactDelta, TxReport};
pub use schema::{AttrName, AttributeDefinition, Cardinality, Schema, Uniqueness};
pub use store::Store;
pub use tx::{Change, EntityTerm, Expectation, Operation, Tx, TxBuilder, ValueTerm};
pub use value::{FiniteF64, Value, ValueType};
