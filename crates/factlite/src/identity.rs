use std::{fmt, str::FromStr};

use crate::{Error, ErrorCode, ErrorContext, Result};

fn identity_hex(input: &str, prefix: &str, field: &'static str) -> Result<String> {
    let hex = input
        .strip_prefix(prefix)
        .ok_or_else(|| Error::invalid(field, "invalid identity prefix"))?;
    if hex.len() != 32
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid(
            field,
            "identity must contain 32 lowercase hexadecimal digits",
        ));
    }
    Ok(hex.to_owned())
}

macro_rules! identity {
    ($name:ident, $prefix:literal, $field:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);
        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                identity_hex(s, $prefix, $field).map(Self)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

identity!(
    StoreId,
    "s1.",
    "store",
    "A parsed store identity. Parsing does not prove store membership."
);
identity!(
    LineageId,
    "l1.",
    "lineage",
    "A parsed commit-lineage identity."
);

/// An opaque store-bound entity token. Parsing validates syntax, not allocation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityId {
    store: StoreId,
    allocation: u64,
}

impl EntityId {
    pub(crate) fn allocated(store: StoreId, allocation: u64) -> Result<Self> {
        if allocation == 0 {
            return Err(Error::invalid("entity", "allocation zero is reserved"));
        }
        Ok(Self { store, allocation })
    }

    pub(crate) fn allocation(&self) -> u64 {
        self.allocation
    }

    /// Returns the store binding encoded in this identity.
    pub fn store(&self) -> &StoreId {
        &self.store
    }
}

impl FromStr for EntityId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s.split('.');
        if parts.next() != Some("e1") {
            return Err(Error::invalid("entity", "invalid entity prefix"));
        }
        let store = parts
            .next()
            .ok_or_else(|| Error::invalid("entity", "missing store binding"))?;
        let allocation = parts
            .next()
            .ok_or_else(|| Error::invalid("entity", "missing allocation"))?;
        if parts.next().is_some()
            || allocation.len() != 16
            || !allocation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::invalid("entity", "invalid allocation encoding"));
        }
        let allocation = u64::from_str_radix(allocation, 16)
            .map_err(|_| Error::invalid("entity", "invalid allocation"))?;
        if allocation == 0 {
            return Err(Error::invalid("entity", "allocation zero is reserved"));
        }
        Ok(Self {
            store: format!("s1.{store}").parse()?,
            allocation,
        })
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e1.{}.{:016x}", self.store.0, self.allocation)
    }
}

/// A logical committed revision. Zero denotes an empty database.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(u64);

impl Revision {
    /// The empty-store revision.
    pub const ZERO: Self = Self(0);
    /// Constructs a logical revision; a store must still validate availability.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    /// Returns its logical sequence number.
    pub const fn get(self) -> u64 {
        self.0
    }
    /// Advances a revision without wrapping at the numeric boundary.
    pub fn next(self) -> Result<Self> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| Error::invalid("revision", "revision space exhausted"))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Revision {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        if s.is_empty()
            || !s.bytes().all(|b| b.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return Err(Error::invalid(
                "revision",
                "revision must be canonical unsigned decimal",
            ));
        }
        s.parse()
            .map(Self)
            .map_err(|_| Error::invalid("revision", "revision out of range"))
    }
}

/// An immutable store, lineage, and revision token with encoding version one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SnapshotId {
    store: StoreId,
    lineage: LineageId,
    revision: Revision,
}

impl SnapshotId {
    /// Constructs a token. Availability and membership require store validation.
    pub fn new(store: StoreId, lineage: LineageId, revision: Revision) -> Self {
        Self {
            store,
            lineage,
            revision,
        }
    }
    /// Returns its encoding version.
    pub const fn version(&self) -> u32 {
        1
    }
    /// Returns its store binding.
    pub fn store(&self) -> &StoreId {
        &self.store
    }
    /// Returns its commit-lineage binding.
    pub fn lineage(&self) -> &LineageId {
        &self.lineage
    }
    /// Returns its pinned revision.
    pub fn revision(&self) -> Revision {
        self.revision
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "d1.{}.{}.{}",
            self.store.0, self.lineage.0, self.revision
        )
    }
}

impl FromStr for SnapshotId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let parse = || -> Result<Self> {
            let parts: Vec<_> = s.split('.').collect();
            if parts.len() != 4 || parts[0] != "d1" {
                return Err(Error::invalid("snapshot", "invalid snapshot encoding"));
            }
            Ok(Self::new(
                format!("s1.{}", parts[1]).parse()?,
                format!("l1.{}", parts[2]).parse()?,
                parts[3].parse()?,
            ))
        };
        parse().map_err(|err| {
            Error::new(
                ErrorCode::InvalidSnapshot,
                ErrorContext::InvalidInput { field: "snapshot" },
                err.to_string(),
            )
        })
    }
}

/// A nonempty transaction-local label, distinct from any allocated identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TempId(String);

impl TempId {
    /// Validates a label. Unicode is preserved; empty labels and NUL are rejected.
    pub fn new(label: impl Into<String>) -> Result<Self> {
        let label = label.into();
        if label.is_empty() || label.contains('\0') {
            return Err(Error::invalid(
                "tempid",
                "temporary labels must be nonempty and contain no NUL",
            ));
        }
        Ok(Self(label))
    }
    /// Returns the exact submitted label.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TempId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn malformed_identity_boundaries() {
        for s in [
            "",
            "s2.00000000000000000000000000000000",
            "s1.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "s1.１２３",
        ] {
            assert!(s.parse::<StoreId>().is_err());
        }
        for s in [
            "",
            "e1.00000000000000000000000000000000.0000000000000000",
            "e1.00000000000000000000000000000000.1",
            "e1.00000000000000000000000000000000.0000000000000001.extra",
        ] {
            assert!(s.parse::<EntityId>().is_err());
        }
        for s in ["+1", "-1", "01", " 1", "1 ", "18446744073709551616", ""] {
            assert!(s.parse::<Revision>().is_err());
        }
        assert_eq!(
            Revision::new(u64::MAX).next().unwrap_err().code(),
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            "d2.a.b.0".parse::<SnapshotId>().unwrap_err().code(),
            ErrorCode::InvalidSnapshot
        );
        assert!(TempId::new("").is_err());
        assert!(TempId::new("a\0b").is_err());
        assert_eq!(TempId::new("任务").unwrap().as_str(), "任务");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn entity_roundtrip_and_store_binding(store in any::<u128>(), other in any::<u128>(), allocation in 1_u64..=u64::MAX) {
            let token = format!("e1.{store:032x}.{allocation:016x}");
            let entity: EntityId = token.parse().unwrap();
            prop_assert_eq!(entity.to_string(), token);
            prop_assert_eq!(entity.store().to_string(), format!("s1.{store:032x}"));
            let foreign: EntityId = format!("e1.{other:032x}.{allocation:016x}").parse().unwrap();
            prop_assert_eq!(entity == foreign, store == other);
        }
        #[test]
        fn snapshot_roundtrip(store in any::<u128>(), lineage in any::<u128>(), revision in any::<u64>()) {
            let token = format!("d1.{store:032x}.{lineage:032x}.{revision}");
            let snapshot: SnapshotId = token.parse().unwrap();
            prop_assert_eq!(snapshot.revision(), Revision::new(revision));
            prop_assert_eq!(snapshot.to_string(), token);
            prop_assert_eq!(snapshot.to_string().parse::<SnapshotId>().unwrap(), snapshot);
        }
        #[test]
        fn arbitrary_identity_parsers_never_panic(input in ".{0,256}") {
            let _ = input.parse::<EntityId>(); let _ = input.parse::<StoreId>();
            let _ = input.parse::<SnapshotId>(); let _ = input.parse::<Revision>();
        }
    }
}
