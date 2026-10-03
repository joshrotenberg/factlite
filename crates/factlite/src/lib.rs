//! Typed facts and durable revision-pinned database views over SQLite.
//!
//! Values preserve their type, identities bind their store, and database views
//! use explicit revision tokens. SQLite persistence is added separately.

mod error;
mod identity;
mod value;

pub use error::{Error, ErrorCode, ErrorContext, Result};
pub use identity::{EntityId, LineageId, Revision, SnapshotId, StoreId, TempId};
pub use value::{FiniteF64, Value, ValueType};
