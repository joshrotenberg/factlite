//! Authoritative persisted reports/events and bounded semantic replay.

use crate::{
    Error, ErrorCode, ErrorContext, Fact, FactDelta, Operation, Result, Revision, SnapshotId,
    TxReport,
};
use crate::{
    codec,
    model::Model,
    store::{corrupt, decode_revision, read_head, revision_bytes},
};
use rusqlite::{Connection, Transaction};

#[cfg(test)]
#[path = "replay_profile.rs"]
mod profile;

pub(crate) fn persist_report(
    transaction: &Transaction<'_>,
    report: &TxReport,
    retry: Option<(&str, &crate::CanonicalRequest)>,
) -> Result<()> {
    if read_head(transaction)? != report.before {
        return Err(corrupt(
            "persisted report pre-head does not match transaction basis",
        ));
    }
    let revision = revision_bytes(report.after.revision());
    transaction.execute("INSERT INTO transactions(revision,report,report_encoding,request_version,canonical_version,canonical_request,idempotency_key) VALUES(?1,?2,1,1,?3,?4,?5)",rusqlite::params![revision.as_slice(),codec::encode_report(report),retry.map(|(_,canonical)| i64::from(canonical.version())),retry.map(|(_,canonical)| canonical.bytes()),retry.map(|(key,_)| key)])?;
    for (ordinal, definition) in report.schema_added.iter().enumerate() {
        transaction.execute(
            "INSERT INTO schema_events(revision,ordinal,attribute,definition) VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                revision.as_slice(),
                ordinal as i64,
                definition.name().as_str(),
                codec::encode_definition(definition)
            ],
        )?;
    }
    for (ordinal, entity) in report.created_entities.iter().enumerate() {
        transaction.execute(
            "INSERT INTO entity_events(revision,ordinal,entity) VALUES(?1,?2,?3)",
            rusqlite::params![revision.as_slice(), ordinal as i64, entity.to_string()],
        )?;
    }
    for (ordinal, event) in report.effective_delta.iter().enumerate() {
        transaction.execute("INSERT INTO fact_events(revision,ordinal,operation,entity,attribute,type_tag,value) VALUES(?1,?2,?3,?4,?5,?6,?7)",rusqlite::params![revision.as_slice(),ordinal as i64,match event.operation{Operation::Retract=>0,Operation::Assert=>1},event.fact.entity.to_string(),event.fact.attribute.as_str(),codec::type_tag(event.fact.value.value_type()),codec::encode_value(&event.fact.value)])?;
    }
    crate::index::apply(transaction, report)?;
    let updated = transaction.execute(
        "UPDATE store_info SET head=?1 WHERE singleton=1",
        [revision.as_slice()],
    )?;
    if updated != 1 {
        return Err(corrupt("store head disappeared while committing"));
    }
    Ok(())
}

pub(crate) fn replay(connection: &Connection, basis: &SnapshotId) -> Result<Model> {
    replay_impl::<Incremental>(connection, basis)
}

// Retain the pre-7.3 and pre-9.4 path for differential tests: full-state
// transitions, and keyed records evaluated then reapplied from their report.
// Both paths perform exactly the same format, identity, event and
// canonical-request checks.
#[cfg(test)]
pub(crate) fn replay_reference(connection: &Connection, basis: &SnapshotId) -> Result<Model> {
    replay_impl::<Reference>(connection, basis)
}

/// How replay advances its model by one retained record.
trait Transitions {
    fn unkeyed(model: Model, report: &TxReport) -> Result<Model>;
    fn keyed(model: Model, request: &crate::Tx, report: &TxReport) -> Result<Model>;
}

fn retained_report(evaluated: Result<(Model, TxReport)>, report: &TxReport) -> Result<Model> {
    let (evaluated, expected) = evaluated.map_err(|error| {
        corrupt(format!(
            "retained retry request cannot produce its report: {error}"
        ))
    })?;
    if &expected != report {
        return Err(corrupt("retained retry request and report disagree"));
    }
    Ok(evaluated)
}

/// Incremental transitions in place (9.4). A keyed record's evaluated state is
/// kept after exact report equality (7.3): evaluation has validated the batch
/// and produced canonical declarations, allocations and effective
/// differences, so reapplying the identical report adds no evidence.
struct Incremental;

impl Transitions for Incremental {
    fn unkeyed(model: Model, report: &TxReport) -> Result<Model> {
        model.apply_report_owned(report)
    }
    fn keyed(model: Model, request: &crate::Tx, report: &TxReport) -> Result<Model> {
        retained_report(model.evaluate_owned(request), report)
    }
}

/// Full-state transitions, with keyed records evaluated and then reapplied.
#[cfg(test)]
struct Reference;

#[cfg(test)]
impl Transitions for Reference {
    fn unkeyed(model: Model, report: &TxReport) -> Result<Model> {
        model.apply_report_reference(report)
    }
    fn keyed(model: Model, request: &crate::Tx, report: &TxReport) -> Result<Model> {
        drop(retained_report(model.evaluate_reference(request), report)?);
        model.apply_report_reference(report)
    }
}

fn replay_impl<T: Transitions>(connection: &Connection, basis: &SnapshotId) -> Result<Model> {
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
    let zero = SnapshotId::new(head.store().clone(), head.lineage().clone(), Revision::ZERO);
    let mut model = Model::empty(zero)?;
    let mut statement=connection.prepare("SELECT revision,report,report_encoding,request_version,canonical_version,canonical_request,idempotency_key FROM transactions WHERE revision<=?1 ORDER BY revision")?;
    let rows = statement.query_map([revision_bytes(basis.revision()).as_slice()], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<Vec<u8>>>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    for row in rows {
        let (revision, bytes, encoding, request_version, canonical_version, canonical_request, key) =
            row?;
        if encoding != 1 || request_version != 1 {
            return Err(Error::new(
                ErrorCode::UnsupportedFormat,
                ErrorContext::InvalidInput {
                    field: "transaction encoding",
                },
                "unsupported retained transaction encoding",
            ));
        }
        let report = codec::decode_report(&bytes)?;
        if decode_revision(&revision)? != report.after.revision() {
            return Err(corrupt("transaction row revision differs from report"));
        }
        check_events(connection, &revision, &report)?;
        model = match (key, canonical_version, canonical_request) {
            (None, None, None) => T::unkeyed(model, &report)?,
            (Some(_), Some(version), Some(bytes)) => {
                let request = crate::canonical_decode::decode_request(version, &bytes)?;
                T::keyed(model, &request, &report)?
            }
            _ => return Err(corrupt("incomplete retained retry evidence")),
        };
    }
    if &model.catalog.snapshot != basis {
        return Err(corrupt("retained log does not reach requested revision"));
    }
    Ok(model)
}

pub(crate) fn retry_report(
    connection: &Connection,
    key: &str,
    canonical: &crate::CanonicalRequest,
) -> Result<Option<TxReport>> {
    use rusqlite::OptionalExtension;
    let row = connection.query_row("SELECT revision,report,report_encoding,request_version,canonical_version,canonical_request FROM transactions WHERE idempotency_key=?1",[key],|row| Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?,row.get::<_,i64>(4)?,row.get::<_,Vec<u8>>(5)?))).optional()?;
    let Some((revision, report, encoding, request_version, version, bytes)) = row else {
        return Ok(None);
    };
    crate::canonical_decode::decode_request(version, &bytes)?;
    if version != i64::from(canonical.version()) || bytes != canonical.bytes() {
        return Err(Error::new(
            ErrorCode::IdempotencyConflict,
            ErrorContext::InvalidInput {
                field: "idempotency_key",
            },
            "retry key already belongs to a different submitted request",
        ));
    }
    if encoding != 1 || request_version != 1 {
        return Err(Error::new(
            ErrorCode::UnsupportedFormat,
            ErrorContext::InvalidInput {
                field: "transaction encoding",
            },
            "unsupported retained transaction encoding",
        ));
    }
    let report = codec::decode_report(&report)?;
    if decode_revision(&revision)? != report.after.revision() {
        return Err(corrupt("retry revision differs from original report"));
    }
    Ok(Some(report))
}

fn check_events(connection: &Connection, revision: &[u8], report: &TxReport) -> Result<()> {
    let definitions=connection.prepare("SELECT ordinal,attribute,definition FROM schema_events WHERE revision=?1 ORDER BY ordinal")?.query_map([revision],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if definitions.len() != report.schema_added.len() {
        return Err(corrupt("schema events differ from report"));
    }
    for ((ordinal, name, bytes), (expected_ordinal, expected)) in definitions
        .into_iter()
        .zip(report.schema_added.iter().enumerate())
    {
        if ordinal != expected_ordinal as i64
            || name != expected.name().as_str()
            || codec::decode_definition(&bytes)? != *expected
        {
            return Err(corrupt("schema event ordinal or definition mismatch"));
        }
    }
    let entities = connection
        .prepare("SELECT ordinal,entity FROM entity_events WHERE revision=?1 ORDER BY ordinal")?
        .query_map([revision], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if entities.len() != report.created_entities.len() {
        return Err(corrupt("entity events differ from report"));
    }
    for ((ordinal, id), (expected_ordinal, expected)) in entities
        .into_iter()
        .zip(report.created_entities.iter().enumerate())
    {
        if ordinal != expected_ordinal as i64 || id != expected.to_string() {
            return Err(corrupt("entity event ordinal or identity mismatch"));
        }
    }
    let events=connection.prepare("SELECT ordinal,operation,entity,attribute,type_tag,value FROM fact_events WHERE revision=?1 ORDER BY ordinal")?.query_map([revision],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,i64>(4)?,row.get::<_,Vec<u8>>(5)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if events.len() != report.effective_delta.len() {
        return Err(corrupt("fact events differ from report"));
    }
    for ((ordinal, operation, entity, attribute, tag, bytes), (expected_ordinal, expected)) in
        events
            .into_iter()
            .zip(report.effective_delta.iter().enumerate())
    {
        let operation = match operation {
            0 => Operation::Retract,
            1 => Operation::Assert,
            _ => return Err(corrupt("invalid persisted operation")),
        };
        let entity = entity
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))?;
        let attribute = attribute
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))?;
        let tag = u8::try_from(tag).map_err(|_| corrupt("invalid persisted type tag"))?;
        let value = codec::decode_value(tag, &bytes)?;
        if ordinal != expected_ordinal as i64
            || (FactDelta {
                operation,
                fact: Fact {
                    entity,
                    attribute,
                    value,
                },
            }) != *expected
        {
            return Err(corrupt("fact event ordinal or value mismatch"));
        }
    }
    Ok(())
}

// Called only inside Db's read transaction after validating/replaying its basis.
pub(crate) fn reports_between(
    connection: &Connection,
    after: Revision,
    basis: Revision,
) -> Result<Vec<TxReport>> {
    let mut statement = connection.prepare("SELECT revision,report FROM transactions WHERE revision>?1 AND revision<=?2 ORDER BY revision")?;
    let rows = statement.query_map(
        rusqlite::params![
            revision_bytes(after).as_slice(),
            revision_bytes(basis).as_slice()
        ],
        |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let mut reports = Vec::new();
    let mut expected = after;
    for row in rows {
        let (revision, bytes) = row?;
        let report = codec::decode_report(&bytes)?;
        if decode_revision(&revision)? != report.after.revision() {
            return Err(corrupt("transaction row revision differs from report"));
        }
        // Retained history is contiguous; a gap means missing transactions.
        expected = expected.next()?;
        if report.after.revision() != expected {
            return Err(corrupt("retained transaction log has a gap"));
        }
        reports.push(report);
    }
    // A missing tail cannot reach here: reads first check that their basis
    // revision is retained (`index::check_basis`).
    Ok(reports)
}

pub(crate) fn verify_foreign_keys(connection: &Connection) -> Result<()> {
    if connection
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(corrupt("retained event foreign keys are inconsistent"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cardinality, Store, TempId, Tx, Uniqueness, Value, ValueType,
        model::tests::{attr, def},
    };
    use proptest::prelude::*;
    use rusqlite::TransactionBehavior;

    fn write(store: &Store, tx: &Tx) -> (Model, TxReport) {
        let mut connection = store.connection().unwrap();
        let sql = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let model = replay(&sql, &read_head(&sql).unwrap()).unwrap();
        let (next, report) = model.evaluate(tx).unwrap();
        persist_report(&sql, &report, None).unwrap();
        sql.commit().unwrap();
        (next, report)
    }
    fn create(store: &Store) -> (Model, TxReport) {
        let label = TempId::new("entity").unwrap();
        write(
            store,
            &Tx::builder()
                .define_attribute(def(
                    "x/value",
                    ValueType::I64,
                    Cardinality::One,
                    Uniqueness::None,
                ))
                .new_entity(label.clone())
                .assert(label, attr("x/value"), Value::I64(i64::MIN))
                .metadata("source", Value::String("log-test".into()))
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn reopen_replays_original_reports_and_bounded_states() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.sqlite");
        let store = Store::open(&path).unwrap();
        let (a, ra) = create(&store);
        let id = ra.created_entities[0].clone();
        let (b, rb) = write(
            &store,
            &Tx::builder()
                .assert(id.clone(), attr("x/value"), Value::I64(i64::MAX))
                .build()
                .unwrap(),
        );
        let (_, rc) = write(
            &store,
            &Tx::builder()
                .assert(id, attr("x/value"), Value::I64(i64::MAX))
                .metadata("noop", Value::Bool(true))
                .build()
                .unwrap(),
        );
        assert!(rc.effective_delta.is_empty());
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let connection = reopened.connection().unwrap();
        assert_eq!(replay(&connection, &ra.after).unwrap().facts(), a.facts());
        assert_eq!(replay(&connection, &rb.after).unwrap().facts(), b.facts());
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT report FROM transactions WHERE revision=?1",
                [revision_bytes(ra.after.revision()).as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codec::decode_report(&bytes).unwrap(), ra);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM transactions", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    #[test]
    fn mismatched_events_reports_and_missing_rows_fail_reopen() {
        for mutation in [
            "UPDATE fact_events SET value=X'8000000000000000'",
            "UPDATE transactions SET report=X'00'",
            "DELETE FROM fact_events",
            "DELETE FROM entity_events",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("bad.sqlite");
            let store = Store::open(&path).unwrap();
            create(&store);
            drop(store);
            let connection = Connection::open(&path).unwrap();
            // Corruption fixture deliberately bypasses FK protection; production
            // writes never disable it, and reopen must detect these broken rows.
            connection
                .pragma_update(None, "foreign_keys", false)
                .unwrap();
            connection.execute_batch(mutation).unwrap();
            drop(connection);
            assert_eq!(
                Store::open(&path).err().unwrap().code(),
                ErrorCode::Storage,
                "{mutation}"
            );
        }
    }
    #[test]
    fn reports_cannot_claim_noops_or_noncontiguous_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("model.sqlite")).unwrap();
        let (model, report) = create(&store);
        let mut invalid = report.clone();
        invalid.after = SnapshotId::new(
            report.after.store().clone(),
            report.after.lineage().clone(),
            Revision::new(2),
        );
        let zero = Model::empty(report.before.clone()).unwrap();
        assert!(zero.apply_report(&invalid).is_err());
        let (mut next, mut noop) = model.evaluate(&Tx::default()).unwrap();
        noop.effective_delta = report.effective_delta;
        assert!(model.apply_report(&noop).is_err());
        next.set_facts(Default::default());
        assert!(
            next.catalog
                .require_known(&report.created_entities[0])
                .is_ok()
        );
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn persisted_revision_sequences_equal_reference_evaluation(values in proptest::collection::vec(-3_i64..4,0..8)){
            let dir=tempfile::tempdir().unwrap();let store=Store::open(dir.path().join("property.sqlite")).unwrap();let(mut model,report)=create(&store);let id=report.created_entities[0].clone();let mut snapshots=vec![model.clone()];
            for value in values{let request=Tx::builder().assert(id.clone(),attr("x/value"),Value::I64(value)).build().unwrap();let(expected,_)=model.evaluate(&request).unwrap();let(next,_)=write(&store,&request);prop_assert_eq!(&next.facts(),&expected.facts());model=expected;snapshots.push(model.clone());}
            let connection=store.connection().unwrap();for expected in snapshots{let actual=replay(&connection,&expected.catalog.snapshot).unwrap();prop_assert_eq!(actual.facts(),expected.facts());prop_assert_eq!(actual.catalog.schema,expected.catalog.schema);prop_assert_eq!(actual.catalog.entities,expected.catalog.entities);}
        }
    }
}
