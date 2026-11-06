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
/// `asserted <= R` and the fact is not retracted at or before R. Type tags
/// 0..=7 are bool, i64, f64, string, bytes, ref, keyword and inst.
pub(crate) fn datoms_table(name: &str) -> String {
    format!(
        "CREATE TABLE {name} (
 entity TEXT NOT NULL REFERENCES entity_events(entity),
 attribute TEXT NOT NULL REFERENCES schema_events(attribute),
 type_tag INTEGER NOT NULL CHECK(type_tag BETWEEN 0 AND 7),
 value BLOB NOT NULL,
 asserted BLOB NOT NULL REFERENCES transactions(revision)
  CHECK(typeof(asserted)='blob' AND length(asserted)=8),
 retracted BLOB REFERENCES transactions(revision)
  CHECK(retracted IS NULL OR (typeof(retracted)='blob' AND length(retracted)=8 AND retracted>asserted)),
 PRIMARY KEY(entity,attribute,type_tag,value,asserted)
) WITHOUT ROWID;"
    )
}

/// The partial unique index allows one open interval per fact.
pub(crate) const OPEN_DDL: &str = "CREATE UNIQUE INDEX datoms_open ON datoms(entity,attribute,type_tag,value) WHERE retracted IS NULL;";

/// Format 3 adds the attribute/value index, which serves revision-bounded
/// reads by attribute or by attribute and value without the primary key's
/// entity prefix. An attribute fixes its value type, so the type tag is
/// not part of it.
pub(crate) const AVET_DDL: &str =
    "CREATE INDEX datoms_avet ON datoms(attribute,value,entity,asserted,retracted);";

/// The complete current datom table and indexes.
pub(crate) fn ddl() -> String {
    format!("{}\n{OPEN_DDL}\n{AVET_DDL}", datoms_table("datoms"))
}

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
    let stored_types = types(&schema);
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
        facts.insert(fact_row(&stored_types, entity, attribute, tag, &bytes)?);
    }
    Ok(Model::new(
        Catalog::retained(basis.clone(), schema, entities),
        facts,
    ))
}

/// Fails unless `basis` names this store and lineage at a retained revision.
/// Every read checks this inside its own SQL snapshot before reading.
pub(crate) fn check_basis(connection: &Connection, basis: &SnapshotId) -> Result<()> {
    use rusqlite::OptionalExtension;
    let head = read_head(connection)?;
    // The revision must be retained, not just at or below head: a structural
    // open does not prove that history is contiguous.
    let retained = basis.revision() == Revision::ZERO
        || connection
            .query_row(
                "SELECT 1 FROM transactions WHERE revision=?1",
                [revision_bytes(basis.revision()).as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
    if head.store() != basis.store()
        || head.lineage() != basis.lineage()
        || basis.revision() > head.revision()
        || !retained
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
    types: &StoredTypes,
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
        fact_row(types, entity, attribute, tag, &bytes)
    })
    .collect()
}

/// Stored type tag of each attribute visible at a basis, loaded once per
/// operation.
pub(crate) type StoredTypes = BTreeMap<String, u8>;

pub(crate) fn stored_types(connection: &Connection, basis: &SnapshotId) -> Result<StoredTypes> {
    Ok(types(&definitions_at(connection, basis)?))
}

/// Fails unless `fact` has an attribute visible in `types`, with that type.
pub(crate) fn check_fact_type(types: &StoredTypes, fact: &Fact) -> Result<()> {
    if types.get(fact.attribute.as_str()) != Some(&codec::type_tag(fact.value.value_type())) {
        return Err(corrupt(
            "stored fact type or attribute differs from the visible schema",
        ));
    }
    Ok(())
}

fn types(schema: &Definitions) -> StoredTypes {
    schema
        .iter()
        .map(|(name, (definition, _))| {
            (
                name.as_str().to_owned(),
                codec::type_tag(definition.value_type()),
            )
        })
        .collect()
}

/// Decodes a stored fact, failing unless its attribute is visible at the basis
/// and its type tag is the attribute's type. Full validation at open also
/// guarantees this; checking here keeps structurally opened views from passing
/// a mistyped value to code that relies on schema types.
fn fact_row(
    types: &StoredTypes,
    entity: String,
    attribute: String,
    tag: i64,
    bytes: &[u8],
) -> Result<Fact> {
    let tag = u8::try_from(tag).map_err(|_| corrupt("invalid stored type tag"))?;
    if types.get(&attribute) != Some(&tag) {
        return Err(corrupt(
            "stored fact type or attribute differs from the visible schema",
        ));
    }
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

/// Effective fact events in `(after, basis]` matching each supplied
/// restriction, in revision/ordinal order. Every event is an interval
/// endpoint: an assertion at `asserted` or a retraction at `retracted`. The
/// join finds each endpoint's original ordinal within its revision's events.
/// Intervals equal the retained events (verified at open), so this equals
/// filtering every report in the range.
pub(crate) fn history_matching(
    connection: &Connection,
    basis: &SnapshotId,
    after: Revision,
    entity: Option<&EntityId>,
    attribute: Option<&AttrName>,
    value: Option<&Value>,
) -> Result<Vec<crate::FactEvent>> {
    let mut bindings: Vec<rusqlite::types::Value> = vec![
        revision_bytes(after).to_vec().into(),
        revision_bytes(basis.revision()).to_vec().into(),
    ];
    let mut filters = String::new();
    if let Some(entity) = entity {
        bindings.push(entity.to_string().into());
        filters.push_str(&format!(" AND d.entity=?{}", bindings.len()));
    }
    if let Some(attribute) = attribute {
        bindings.push(attribute.as_str().to_owned().into());
        filters.push_str(&format!(" AND d.attribute=?{}", bindings.len()));
    }
    if let Some(value) = value {
        bindings.push(i64::from(codec::type_tag(value.value_type())).into());
        filters.push_str(&format!(" AND d.type_tag=?{}", bindings.len()));
        bindings.push(codec::encode_value(value).into());
        filters.push_str(&format!(" AND d.value=?{}", bindings.len()));
    }
    let endpoint = |column: &str, operation: u8| {
        format!(
            "SELECT e.revision,e.ordinal,e.operation,d.entity,d.attribute,d.type_tag,d.value \
             FROM datoms d JOIN fact_events e ON e.revision=d.{column} AND e.operation={operation} \
             AND e.entity=d.entity AND e.attribute=d.attribute AND e.type_tag=d.type_tag AND e.value=d.value \
             WHERE d.{column}>?1 AND d.{column}<=?2{filters}"
        )
    };
    let sql = format!(
        "{} UNION ALL {} ORDER BY 1,2",
        endpoint("asserted", 1),
        endpoint("retracted", 0)
    );
    let stored_types = types(&definitions_at(connection, basis)?);
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(&bindings), |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    rows.map(|row| {
        let (revision, ordinal, operation, entity, attribute, tag, bytes) = row?;
        Ok(crate::FactEvent {
            revision: decode_revision(&revision)?,
            ordinal: u64::try_from(ordinal).map_err(|_| corrupt("invalid event ordinal"))?,
            operation: match operation {
                0 => Operation::Retract,
                1 => Operation::Assert,
                _ => return Err(corrupt("invalid persisted operation")),
            },
            fact: fact_row(&stored_types, entity, attribute, tag, &bytes)?,
        })
    })
    .collect()
}

/// The state a commit of `tx` at `head` can observe, and nothing else: the
/// whole schema, the identities `tx` names, the next allocation, every visible
/// value of each entity/attribute pair `tx` changes or expects, and every
/// current owner of each unique value it asserts. Evaluation reads only those,
/// so evaluating `tx` on this model equals evaluating it on the full state.
pub(crate) fn model_for_request(
    connection: &Connection,
    head: &SnapshotId,
    tx: &crate::Tx,
) -> Result<Model> {
    use crate::{EntityTerm, ValueTerm};
    use std::collections::BTreeSet;
    check_basis(connection, head)?;
    let schema = definitions_at(connection, head)?;
    let stored = types(&schema);
    let mut named = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    let mut owned = Vec::new();
    let mut facts = HashSet::new();
    // A lookup's owner at head, with the fact that makes it the owner. Lookups
    // that evaluation will reject (unknown or non-unique attribute, wrong
    // type, no owner) load nothing and resolve to nothing here, so the
    // evaluator reports them in its own order.
    let resolve = |attribute: &AttrName,
                   value: &Value,
                   facts: &mut HashSet<Fact>|
     -> Result<Option<EntityId>> {
        let Some((definition, _)) = schema.get(attribute) else {
            return Ok(None);
        };
        if definition.uniqueness() != crate::Uniqueness::UniqueOne
            || definition.value_type() != value.value_type()
        {
            return Ok(None);
        }
        let owners = facts_matching(
            connection,
            head,
            &stored,
            None,
            Some(attribute),
            Some(value),
        )?;
        let owner = owners.iter().map(|fact| fact.entity.clone()).min();
        facts.extend(owners);
        Ok(owner)
    };
    for change in &tx.changes {
        let entity = match &change.entity {
            EntityTerm::Id(id) => Some(id.clone()),
            EntityTerm::Temp(_) => None,
            EntityTerm::Lookup(attribute, value) => resolve(attribute, value, &mut facts)?,
        };
        if let Some(id) = entity {
            named.insert(id.clone());
            pairs.insert((id, change.attribute.clone()));
        }
        let value = match &change.value {
            ValueTerm::Value(value) => Some(value.clone()),
            ValueTerm::TempRef(_) => None,
            ValueTerm::LookupRef(attribute, value) => {
                resolve(attribute, value, &mut facts)?.map(Value::Ref)
            }
        };
        if let Some(value) = &value {
            if let Value::Ref(id) = value {
                named.insert(id.clone());
            }
            let unique = schema
                .get(&change.attribute)
                .is_some_and(|(definition, _)| {
                    definition.uniqueness() == crate::Uniqueness::UniqueOne
                });
            if unique && change.operation == Operation::Assert {
                owned.push((change.attribute.clone(), value.clone()));
            }
        }
    }
    for expectation in &tx.expectations {
        named.insert(expectation.entity.clone());
        pairs.insert((expectation.entity.clone(), expectation.attribute.clone()));
        for value in &expectation.values {
            if let Value::Ref(id) = value {
                named.insert(id.clone());
            }
        }
    }
    let mut entities = BTreeMap::new();
    for id in named {
        if let Some(created) = created(connection, &id)? {
            entities.insert(id, created);
        }
    }
    for (entity, attribute) in &pairs {
        facts.extend(facts_matching(
            connection,
            head,
            &stored,
            Some(entity),
            Some(attribute),
            None,
        )?);
    }
    for (attribute, value) in &owned {
        facts.extend(facts_matching(
            connection,
            head,
            &stored,
            None,
            Some(attribute),
            Some(value),
        )?);
    }
    Ok(Model::new(
        Catalog::partial(head.clone(), schema, entities, next_allocation(connection)?),
        facts,
    ))
}

/// One past the greatest allocation. Identities of one store share a prefix
/// and a fixed-width hex allocation, so text order is allocation order.
fn next_allocation(connection: &Connection) -> Result<Option<u64>> {
    use rusqlite::OptionalExtension;
    let last: Option<String> = connection
        .query_row(
            "SELECT entity FROM entity_events ORDER BY entity DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match last {
        None => Some(1),
        Some(last) => last
            .parse::<EntityId>()
            .map_err(|err| corrupt(err.to_string()))?
            .allocation()
            .checked_add(1),
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
