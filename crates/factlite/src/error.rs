use std::fmt;

use crate::{AttrName, EntityId, Fact, SnapshotId, Value, ValueType};

/// A library operation's result.
pub type Result<T> = std::result::Result<T, Error>;

/// Outcome evidence for an unsuccessful commit call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    /// This request did not commit; rollback/no-write is confirmed.
    RolledBack,
    /// The commit outcome could not be confirmed. Retrying requires the same key
    /// and request; never silently reformulate intent.
    Unknown,
}

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
    /// Storage operation context; the Rust source error remains attached.
    Storage {
        /// Operation that failed.
        operation: &'static str,
    },
    /// Exact-head fencing failure.
    HeadMismatch {
        /// Caller-supplied fence.
        expected: SnapshotId,
        /// Committed pre-head.
        actual: SnapshotId,
    },
    /// Failed exact attribute-set condition.
    Precondition {
        /// Existing entity.
        entity: EntityId,
        /// Pre-head attribute.
        attribute: AttrName,
        /// Sorted, distinct submitted set.
        expected: Vec<Value>,
        /// Sorted, distinct committed set.
        actual: Vec<Value>,
    },
    /// Final-state duplicate unique ownership.
    UniqueOwnership {
        /// Unique attribute.
        attribute: AttrName,
        /// Exact conflicting typed value.
        value: Value,
        /// All owners, in identity order.
        owners: Vec<EntityId>,
    },
    /// A conflicting fact.
    Fact {
        /// Fully resolved fact.
        fact: Fact,
    },
    /// A conflicting entity/attribute pair.
    EntityAttribute {
        /// Entity identity.
        entity: EntityId,
        /// Qualified attribute name.
        attribute: AttrName,
    },
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
    /// A transaction lookup ref that matched no entity.
    Lookup {
        /// The `unique_one` attribute.
        attribute: AttrName,
        /// The looked-up value.
        value: Value,
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
    context: Box<ErrorContext>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    commit_outcome: Option<CommitOutcome>,
}

impl Error {
    pub(crate) fn new(code: ErrorCode, context: ErrorContext, message: impl Into<String>) -> Self {
        Self {
            code,
            context: Box::new(context),
            message: message.into(),
            source: None,
            commit_outcome: None,
        }
    }

    pub(crate) fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::InvalidRequest,
            ErrorContext::InvalidInput { field },
            message,
        )
    }

    pub(crate) fn io(operation: &'static str, source: std::io::Error) -> Self {
        Self {
            code: ErrorCode::Storage,
            context: Box::new(ErrorContext::Storage { operation }),
            message: format!("{operation}: {source}"),
            source: Some(Box::new(source)),
            commit_outcome: None,
        }
    }

    pub(crate) fn storage(operation: &'static str, source: rusqlite::Error) -> Self {
        let code = match &source {
            rusqlite::Error::SqliteFailure(err, _)
                if matches!(
                    err.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                ErrorCode::Busy
            }
            _ => ErrorCode::Storage,
        };
        Self {
            code,
            context: Box::new(ErrorContext::Storage { operation }),
            message: format!("{operation}: {source}"),
            source: Some(Box::new(source)),
            commit_outcome: None,
        }
    }

    /// Returns the stable failure classification.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns typed details about the failure.
    pub fn context(&self) -> &ErrorContext {
        &self.context
    }

    /// Returns commit outcome evidence, or None for a non-commit operation.
    pub fn commit_outcome(&self) -> Option<CommitOutcome> {
        self.commit_outcome
    }

    pub(crate) fn with_commit_outcome(mut self, outcome: CommitOutcome) -> Self {
        self.commit_outcome = Some(outcome);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<rusqlite::Error> for Error {
    fn from(source: rusqlite::Error) -> Self {
        Self::storage("sqlite", source)
    }
}
