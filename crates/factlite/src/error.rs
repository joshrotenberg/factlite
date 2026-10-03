use std::fmt;

use crate::{AttrName, EntityId, ValueType};

/// A library operation's result.
pub type Result<T> = std::result::Result<T, Error>;

/// Stable machine-readable failure classifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// Malformed or structurally invalid input.
    InvalidRequest,
    /// An attribute is absent from the applicable schema.
    UnknownAttribute,
    /// Typed values or positions are incompatible.
    TypeMismatch,
    /// An entity is unknown or belongs to another store.
    UnknownEntity,
    /// Declarative operations conflict.
    ConflictingBatch,
    /// A unique value has multiple owners.
    UniqueViolation,
    /// An exact attribute-set expectation failed.
    PreconditionFailed,
    /// The expected head differs from the committed head.
    HeadMismatch,
    /// A retry key was reused for a different request.
    IdempotencyConflict,
    /// A snapshot is malformed, foreign, or unavailable.
    InvalidSnapshot,
    /// The bounded SQLite lock wait expired.
    Busy,
    /// A storage operation failed.
    Storage,
    /// The stored or submitted format is unsupported.
    UnsupportedFormat,
}

/// Typed details attached to a failure; callers need not parse its message.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorContext {
    /// An attribute that could not be resolved or declared.
    Attribute {
        /// Qualified attribute name.
        attribute: AttrName,
    },
    /// An unallocated, not-yet-created, or foreign entity.
    Entity {
        /// Rejected entity identity.
        entity: EntityId,
    },
    /// The invalid input field.
    InvalidInput {
        /// Field identifying the input boundary.
        field: &'static str,
    },
    /// Expected and actual value types.
    TypeMismatch {
        /// Required type.
        expected: ValueType,
        /// Supplied type.
        actual: ValueType,
    },
}

/// A structured failure with a stable code and human-readable explanation.
#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
    context: ErrorContext,
    message: String,
}

impl Error {
    pub(crate) fn new(code: ErrorCode, context: ErrorContext, message: impl Into<String>) -> Self {
        Self {
            code,
            context,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::InvalidRequest,
            ErrorContext::InvalidInput { field },
            message,
        )
    }

    /// Returns the stable failure classification.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns typed details about the failure.
    pub fn context(&self) -> &ErrorContext {
        &self.context
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
