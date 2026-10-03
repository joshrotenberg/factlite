//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persistence is added separately.

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Private catalog is implemented and tested before evaluator integration in item 1.4."
    )
)]
mod catalog;
mod error;
mod identity;
mod schema;
mod value;

pub use error::{Error, ErrorCode, ErrorContext, Result};
pub use identity::{EntityId, LineageId, Revision, SnapshotId, StoreId, TempId};
pub use schema::{AttrName, AttributeDefinition, Cardinality, Schema, Uniqueness};
pub use value::{FiniteF64, Value, ValueType};
