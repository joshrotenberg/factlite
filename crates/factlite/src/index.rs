//! Derived revision-interval fact index. The retained log stays authoritative:
//! every interval is a deterministic function of retained fact events, written
//! in the same SQLite transaction as its commit and verified on every open.

use std::collections::{BTreeMap, HashSet};

use rusqlite::Connection;

use crate::{
    AttrName, AttributeDefinition, EntityId, Error, ErrorCode, ErrorContext, Fact, Operation,
    Result, Revision, Schema, SnapshotId, TxReport, Value,
    catalog::Catalog,
    codec,
    model::Model,
    store::{corrupt, decode_revision, read_head, revision_bytes},
};

/// One interval per effective assertion: visible at R when
/// `asserted <= R` and the fact is not retracted at or before R.
pub(crate) const DDL: &str = "
CREATE TABLE datoms (
 entity TEXT NOT NULL REFERENCES entity_events(entity),
 attribute TEXT NOT NULL REFERENCES schema_events(attribute),
 type_tag INTEGER NOT NULL CHECK(type_tag BETWEEN 0 AND 5),
 value BLOB NOT NULL,
 asserted BLOB NOT NULL REFERENCES transactions(revision)
  CHECK(typeof(asserted)='blob' AND length(asserted)=8),
 retracted BLOB REFERENCES transactions(revision)
  CHECK(retracted IS NULL OR (typeof(retracted)='blob' AND length(retracted)=8 AND retracted>asserted)),
 PRIMARY KEY(entity,attribute,type_tag,value,asserted)
) WITHOUT ROWID;
CREATE UNIQUE INDEX datoms_open ON datoms(entity,attribute,type_tag,value) WHERE retracted IS NULL;
CREATE INDEX datoms_avet ON datoms(attribute,value,entity,asserted,retracted);";

/// Format 3 adds the attribute/value index, which serves revision-bounded
/// reads by attribute or by attribute and value without the primary key's
/// entity prefix. An attribute fixes its value type, so the type tag is
/// not part of it.
pub(crate) const AVET_DDL: &str =
    "CREATE INDEX datoms_avet ON datoms(attribute,value,entity,asserted,retracted);";

/// Indexes on `datoms` that open requires, in name order.
pub(crate) const INDEXES: [&str; 2] = ["datoms_avet", "datoms_open"];

/// Stored interval row in primary-key order. SQLite's BINARY text collation and
/// BLOB comparison are bytewise, matching Rust's `String`/`Vec<u8>` ordering.
type Interval = (String, String, i64, Vec<u8>, Vec<u8>, Option<Vec<u8>>);

/// Applies one report's effective delta. Called by `persist_report` inside the
/// commit transaction, so a failed commit leaves no interval changes.
pub(crate) fn apply(connection: &Connection, report: &TxReport) -> Result<()> {
    let revision = revision_bytes(report.after.revision());
    for delta in &report.effective_delta {
        let fact = &delta.fact;
        let entity = fact.entity.to_string();
        let tag = codec::type_tag(fact.value.value_type());
        let value = codec::encode_value(&fact.value);
        match delta.operation {
            Operation::Retract => {
                let closed = connection.execute(
                    "UPDATE datoms SET retracted=?1 WHERE entity=?2 AND attribute=?3 AND type_tag=?4 AND value=?5 AND retracted IS NULL",
                    rusqlite::params![revision.as_slice(), entity, fact.attribute.as_str(), tag, value],
                )?;
                if closed != 1 {
                    return Err(corrupt("retraction has no open datom interval"));
                }
            }
            Operation::Assert => {
                connection.execute(
                    "INSERT INTO datoms(entity,attribute,type_tag,value,asserted,retracted) VALUES(?1,?2,?3,?4,?5,NULL)",
                    rusqlite::params![entity, fact.attribute.as_str(), tag, value, revision.as_slice()],
                )?;
            }
        }
    }
    Ok(())
}

/// Derives the expected interval rows from retained fact events alone.
fn derive(connection: &Connection) -> Result<Vec<Interval>> {
    let mut statement = connection.prepare(
        "SELECT revision,operation,entity,attribute,type_tag,value FROM fact_events ORDER BY revision,ordinal",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Vec<u8>>(5)?,
        ))
    })?;
    let mut open = BTreeMap::new();
    let mut intervals = Vec::new();
    for row in rows {
        let (revision, operation, entity, attribute, tag, value) = row?;
        let key = (entity, attribute, tag, value);
        match operation {
            1 => {
                if open.insert(key, revision).is_some() {
                    return Err(corrupt("fact events assert an already visible fact"));
                }
            }
            0 => {
                let asserted = open
                    .remove(&key)
                    .ok_or_else(|| corrupt("fact events retract an absent fact"))?;
                let (entity, attribute, tag, value) = key;
                intervals.push((entity, attribute, tag, value, asserted, Some(revision)));
            }
            _ => return Err(corrupt("invalid persisted operation")),
        }
    }
    intervals.extend(
        open.into_iter()
            .map(|((entity, attribute, tag, value), asserted)| {
                (entity, attribute, tag, value, asserted, None)
            }),
    );
    intervals.sort();
    Ok(intervals)
}

fn stored(connection: &Connection) -> Result<Vec<Interval>> {
    Ok(connection
        .prepare("SELECT entity,attribute,type_tag,value,asserted,retracted FROM datoms ORDER BY entity,attribute,type_tag,value,asserted")?
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?)
}

/// Fails closed unless stored intervals equal those derived from fact events.
pub(crate) fn verify(connection: &Connection) -> Result<()> {
    if stored(connection)? != derive(connection)? {
        return Err(corrupt("datom intervals do not match retained fact events"));
    }
    Ok(())
}

/// Replaces all intervals with those derived from fact events. Used only by the
/// format-1 upgrade, inside its open transaction.
pub(crate) fn rebuild(connection: &Connection) -> Result<()> {
    let intervals = derive(connection)?;
    connection.execute("DELETE FROM datoms", [])?;
    let mut insert = connection.prepare(
        "INSERT INTO datoms(entity,attribute,type_tag,value,asserted,retracted) VALUES(?1,?2,?3,?4,?5,?6)",
    )?;
    for (entity, attribute, tag, value, asserted, retracted) in intervals {
        insert.execute(rusqlite::params![
            entity, attribute, tag, value, asserted, retracted
        ])?;
    }
    Ok(())
}

/// Loads schema, allocations and visible facts at `basis` from retained events
/// and intervals, without replaying transactions. Callers run it inside the
/// operation's single SQLite read or write transaction.
pub(crate) fn model_at(connection: &Connection, basis: &SnapshotId) -> Result<Model> {
    check_basis(connection, basis)?;
    let bound = revision_bytes(basis.revision());
    let schema = definitions_at(connection, basis)?;
    let mut entities = BTreeMap::new();
    for row in connection
        .prepare("SELECT revision,entity FROM entity_events WHERE revision<=?1")?
        .query_map([bound.as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })?
    {
        let (revision, entity) = row?;
        let entity = entity
            .parse()
            .map_err(|err: Error| corrupt(err.to_string()))?;
        entities.insert(entity, decode_revision(&revision)?);
    }
    let mut facts = HashSet::new();
    for row in connection
        .prepare("SELECT entity,attribute,type_tag,value FROM datoms WHERE asserted<=?1 AND (retracted IS NULL OR retracted>?1)")?
        .query_map([bound.as_slice()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?
    {
        let (entity, attribute, tag, bytes) = row?;
        let tag = u8::try_from(tag).map_err(|_| corrupt("invalid stored type tag"))?;
        facts.insert(Fact {
            entity: entity
                .parse()
                .map_err(|err: Error| corrupt(err.to_string()))?,
            attribute: attribute
                .parse()
                .map_err(|err: Error| corrupt(err.to_string()))?,
            value: codec::decode_value(tag, &bytes)?,
        });
    }
    Ok(Model::new(
        Catalog::retained(basis.clone(), schema, entities),
        facts,
    ))
}

/// Fails unless `basis` names this store and lineage at a retained revision.
/// Every read checks this inside its own SQL snapshot before reading.
pub(crate) fn check_basis(connection: &Connection, basis: &SnapshotId) -> Result<()> {
    let head = read_head(connection)?;
    if head.store() != basis.store()
        || head.lineage() != basis.lineage()
        || basis.revision() > head.revision()
    {
        return Err(Error::new(
            ErrorCode::InvalidSnapshot,
            ErrorContext::InvalidInput { field: "snapshot" },
            "snapshot identity or revision is unavailable",
        ));
    }
    Ok(())
}

type Definitions = BTreeMap<AttrName, (AttributeDefinition, Revision)>;

fn definitions_at(connection: &Connection, basis: &SnapshotId) -> Result<Definitions> {
    let mut schema = BTreeMap::new();
    for row in connection
        .prepare("SELECT revision,definition FROM schema_events WHERE revision<=?1")?
        .query_map([revision_bytes(basis.revision()).as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
    {
        let (revision, bytes) = row?;
        let definition = codec::decode_definition(&bytes)?;
        schema.insert(
            definition.name().clone(),
            (definition, decode_revision(&revision)?),
        );
    }
    Ok(schema)
}

/// Definitions visible at `basis`, from schema events alone.
pub(crate) fn schema_at(connection: &Connection, basis: &SnapshotId) -> Result<Schema> {
    Ok(Schema {
        definitions: definitions_at(connection, basis)?
            .into_iter()
            .map(|(name, (definition, _))| (name, definition))
            .collect(),
    })
}

/// The creation revision of an allocated identity of this store, if any.
pub(crate) fn created(connection: &Connection, entity: &EntityId) -> Result<Option<Revision>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT revision FROM entity_events WHERE entity=?1",
            [entity.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|bytes| decode_revision(&bytes))
        .transpose()
}

/// Whether `entity` is allocated in this store at `basis`.
pub(crate) fn known_at(
    connection: &Connection,
    basis: &SnapshotId,
    entity: &EntityId,
) -> Result<bool> {
    Ok(entity.store() == basis.store()
        && created(connection, entity)?.is_some_and(|created| created <= basis.revision()))
}

/// Visible at R: asserted at or before R and not retracted at or before R.
pub(crate) const VISIBLE: &str = "asserted<=?1 AND (retracted IS NULL OR retracted>?1)";

/// Facts visible at `basis` matching each supplied restriction, unordered.
/// Uses the primary key for an entity, the attribute/value index otherwise.
pub(crate) fn facts_matching(
    connection: &Connection,
    basis: &SnapshotId,
    entity: Option<&EntityId>,
    attribute: Option<&AttrName>,
    value: Option<&Value>,
) -> Result<Vec<Fact>> {
    let mut sql = format!("SELECT entity,attribute,type_tag,value FROM datoms WHERE {VISIBLE}");
    let mut bindings: Vec<rusqlite::types::Value> =
        vec![revision_bytes(basis.revision()).to_vec().into()];
    if let Some(entity) = entity {
        bindings.push(entity.to_string().into());
        sql.push_str(&format!(" AND entity=?{}", bindings.len()));
    }
    if let Some(attribute) = attribute {
        bindings.push(attribute.as_str().to_owned().into());
        sql.push_str(&format!(" AND attribute=?{}", bindings.len()));
    }
    if let Some(value) = value {
        bindings.push(i64::from(codec::type_tag(value.value_type())).into());
        sql.push_str(&format!(" AND type_tag=?{}", bindings.len()));
        bindings.push(codec::encode_value(value).into());
        sql.push_str(&format!(" AND value=?{}", bindings.len()));
    }
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(&bindings), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Vec<u8>>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (entity, attribute, tag, bytes) = row?;
        fact_row(entity, attribute, tag, &bytes)
    })
    .collect()
}

fn fact_row(entity: String, attribute: String, tag: i64, bytes: &[u8]) -> Result<Fact> {
    let tag = u8::try_from(tag).map_err(|_| corrupt("invalid stored type tag"))?;
    Ok(Fact {
        entity: entity
            .parse()
            .map_err(|err: Error| corrupt(err.to_string()))?,
        attribute: attribute
            .parse()
            .map_err(|err: Error| corrupt(err.to_string()))?,
        value: codec::decode_value(tag, bytes)?,
    })
}

/// Open-time cross-check: the indexed state at head equals full replay.
pub(crate) fn verify_against(connection: &Connection, replayed: &Model) -> Result<()> {
    let indexed = model_at(connection, &replayed.catalog.snapshot)?;
    if !same_state(&indexed, replayed) {
        return Err(corrupt("indexed state differs from replayed state"));
    }
    Ok(())
}

pub(crate) fn same_state(a: &Model, b: &Model) -> bool {
    a.catalog.snapshot == b.catalog.snapshot
        && a.catalog.schema == b.catalog.schema
        && a.catalog.entities == b.catalog.entities
        && a.catalog.next_allocation() == b.catalog.next_allocation()
        && a.facts() == b.facts()
}
