//! Lookup refs in transaction changes and reference values (plan 12.3):
//! resolution at the pre-state, validation order, collapse with IDs, retry
//! identity, replay on reopen, and wire strictness.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, EntityId, EntityTerm, ErrorCode, ErrorContext,
    Store, TempId, Tx, Uniqueness, Value, ValueTerm, ValueType, wire,
};

fn attr(name: &str) -> AttrName {
    name.parse().unwrap()
}
fn define(name: &str, ty: ValueType, unique: bool) -> AttributeDefinition {
    AttributeDefinition::new(
        attr(name),
        ty,
        Cardinality::One,
        if unique {
            Uniqueness::UniqueOne
        } else {
            Uniqueness::None
        },
    )
    .unwrap()
}
fn key(text: &str) -> Value {
    Value::String(text.into())
}
fn by_key(text: &str) -> EntityTerm {
    EntityTerm::Lookup(attr("run/key"), key(text))
}
fn ref_by_key(text: &str) -> ValueTerm {
    ValueTerm::LookupRef(attr("run/key"), key(text))
}

/// Three runs keyed "a", "b" and "c", with a unique reference `run/target`,
/// a plain status and a non-unique `run/label`.
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    runs: Vec<EntityId>,
}
impl Fixture {
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("lookups.sqlite")
    }
    fn values(&self, entity: &EntityId, attribute: &str) -> Vec<Value> {
        self.store
            .db()
            .unwrap()
            .facts(factlite::FactFilter {
                entity: Some(entity.clone()),
                attribute: Some(attr(attribute)),
                value: None,
            })
            .unwrap()
            .into_iter()
            .map(|fact| fact.value)
            .collect()
    }
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("lookups.sqlite")).unwrap();
    let mut tx = Tx::builder()
        .define_attribute(define("run/key", ValueType::String, true))
        .define_attribute(define("run/target", ValueType::Ref, true))
        .define_attribute(define("run/status", ValueType::String, false))
        .define_attribute(define("run/label", ValueType::String, false));
    for name in ["a", "b", "c"] {
        let label = TempId::new(name).unwrap();
        tx = tx
            .new_entity(label.clone())
            .assert(label, attr("run/key"), key(name));
    }
    let report = store.commit(tx.build().unwrap()).unwrap();
    let runs = ["a", "b", "c"]
        .map(|name| report.tempids[&TempId::new(name).unwrap()].clone())
        .to_vec();
    Fixture { dir, store, runs }
}
fn fails(f: &Fixture, tx: Tx) -> factlite::Error {
    let head = f.store.head().unwrap();
    let error = f.store.commit(tx).unwrap_err();
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "a failed commit writes nothing"
    );
    error
}

#[test]
fn lookups_resolve_in_entity_and_reference_positions() {
    let f = fixture();
    let report = f
        .store
        .commit(
            Tx::builder()
                .assert(by_key("a"), attr("run/status"), key("done"))
                .assert(by_key("b"), attr("run/target"), ref_by_key("a"))
                .build()
                .unwrap(),
        )
        .unwrap();
    // Reports show resolved identities only.
    assert_eq!(report.effective_delta.len(), 2);
    assert_eq!(f.values(&f.runs[0], "run/status"), vec![key("done")]);
    assert_eq!(
        f.values(&f.runs[1], "run/target"),
        vec![Value::Ref(f.runs[0].clone())]
    );
    // Retraction by lookup.
    f.store
        .commit(
            Tx::builder()
                .retract(by_key("a"), attr("run/status"), key("done"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(f.values(&f.runs[0], "run/status").is_empty());
}

#[test]
fn validation_runs_in_order_and_never_upserts() {
    let f = fixture();
    let status = |entity: EntityTerm| {
        Tx::builder()
            .assert(entity, attr("run/status"), key("x"))
            .build()
            .unwrap()
    };
    // 1. The attribute must be visible before the transaction, even when the
    //    same request defines it.
    assert_eq!(
        fails(&f, status(EntityTerm::Lookup(attr("run/nope"), key("a")))).code(),
        ErrorCode::UnknownAttribute
    );
    let same_request = Tx::builder()
        .define_attribute(define("run/slug", ValueType::String, true))
        .assert(
            EntityTerm::Lookup(attr("run/slug"), key("a")),
            attr("run/status"),
            key("x"),
        )
        .build()
        .unwrap();
    assert_eq!(fails(&f, same_request).code(), ErrorCode::UnknownAttribute);
    // 2. It must be unique_one, even when its type is also wrong.
    assert_eq!(
        fails(
            &f,
            status(EntityTerm::Lookup(attr("run/label"), Value::I64(1)))
        )
        .code(),
        ErrorCode::InvalidRequest
    );
    // 3. The value must have the attribute's type.
    assert_eq!(
        fails(
            &f,
            status(EntityTerm::Lookup(attr("run/key"), Value::I64(1)))
        )
        .code(),
        ErrorCode::TypeMismatch
    );
    // 4. No owner: UnknownEntity naming the lookup, for assertions,
    //    retractions and reference values alike.
    let missing = fails(&f, status(by_key("zzz")));
    assert_eq!(missing.code(), ErrorCode::UnknownEntity);
    assert_eq!(
        missing.context(),
        &ErrorContext::Lookup {
            attribute: attr("run/key"),
            value: key("zzz"),
        }
    );
    assert!(
        wire::encode_error(&missing)
            .contains(r#""lookup":{"attr":"run/key","value":{"string":"zzz"}}"#)
    );
    for tx in [
        Tx::builder()
            .retract(by_key("zzz"), attr("run/status"), key("x"))
            .build()
            .unwrap(),
        Tx::builder()
            .assert(f.runs[0].clone(), attr("run/target"), ref_by_key("zzz"))
            .build()
            .unwrap(),
    ] {
        assert_eq!(fails(&f, tx).code(), ErrorCode::UnknownEntity);
    }
}

#[test]
fn lookups_see_only_the_pre_state() {
    let f = fixture();
    // A transfer can name the old owner: "a" moves to run b, and the lookup
    // still means run a, which takes "z".
    f.store
        .commit(
            Tx::builder()
                .assert(by_key("a"), attr("run/key"), key("z"))
                .assert(f.runs[1].clone(), attr("run/key"), key("a"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(f.values(&f.runs[0], "run/key"), vec![key("z")]);
    assert_eq!(f.values(&f.runs[1], "run/key"), vec![key("a")]);
    // A value the same request assigns is not visible to its lookups.
    let assigns = Tx::builder()
        .assert(f.runs[2].clone(), attr("run/key"), key("new"))
        .assert(by_key("new"), attr("run/status"), key("x"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, assigns).code(), ErrorCode::UnknownEntity);
}

#[test]
fn ids_and_lookups_naming_one_entity_collapse_before_conflict_checks() {
    let f = fixture();
    let a = f.runs[0].clone();
    // The same fact through an ID and a lookup is one assertion.
    let report = f
        .store
        .commit(
            Tx::builder()
                .assert(a.clone(), attr("run/status"), key("done"))
                .assert(by_key("a"), attr("run/status"), key("done"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(report.effective_delta.len(), 1);
    // Asserted through one and retracted through the other conflicts.
    let both = Tx::builder()
        .assert(a.clone(), attr("run/label"), key("x"))
        .retract(by_key("a"), attr("run/label"), key("x"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, both).code(), ErrorCode::ConflictingBatch);
    // Two distinct one-valued assertions on the same entity conflict.
    let distinct = Tx::builder()
        .assert(a.clone(), attr("run/status"), key("x"))
        .assert(by_key("a"), attr("run/status"), key("y"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, distinct).code(), ErrorCode::ConflictingBatch);
    // A reference value through a lookup equals the same Ref value.
    let same_ref = Tx::builder()
        .assert(f.runs[1].clone(), attr("run/target"), Value::Ref(a.clone()))
        .assert(f.runs[1].clone(), attr("run/target"), ref_by_key("a"))
        .build()
        .unwrap();
    assert_eq!(f.store.commit(same_ref).unwrap().effective_delta.len(), 1);
}

#[test]
fn unique_reference_values_through_lookups_keep_uniqueness() {
    let f = fixture();
    // Run c targets run a; run b may not take the same unique reference
    // through a lookup, but may take it in a transfer.
    f.store
        .commit(
            Tx::builder()
                .assert(
                    f.runs[2].clone(),
                    attr("run/target"),
                    Value::Ref(f.runs[0].clone()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let collide = Tx::builder()
        .assert(f.runs[1].clone(), attr("run/target"), ref_by_key("a"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, collide).code(), ErrorCode::UniqueViolation);
    f.store
        .commit(
            Tx::builder()
                .retract(by_key("c"), attr("run/target"), ref_by_key("a"))
                .assert(by_key("b"), attr("run/target"), ref_by_key("a"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.values(&f.runs[1], "run/target"),
        vec![Value::Ref(f.runs[0].clone())]
    );
    assert!(f.values(&f.runs[2], "run/target").is_empty());
}

#[test]
fn earlier_errors_win_over_lookup_failures() {
    let f = fixture();
    let fabricated: EntityId = {
        let mut text = f.runs[2].to_string();
        text.replace_range(text.len() - 3.., "fff");
        text.parse().unwrap()
    };
    // Changes fail in submission order: an earlier unknown ID beats a later
    // unmatched lookup, and the reverse.
    let id_first = Tx::builder()
        .assert(fabricated.clone(), attr("run/status"), key("x"))
        .assert(by_key("zzz"), attr("run/status"), key("x"))
        .build()
        .unwrap();
    assert!(matches!(
        fails(&f, id_first).context(),
        ErrorContext::Entity { .. }
    ));
    let lookup_first = Tx::builder()
        .assert(by_key("zzz"), attr("run/status"), key("x"))
        .assert(fabricated, attr("run/status"), key("x"))
        .build()
        .unwrap();
    assert!(matches!(
        fails(&f, lookup_first).context(),
        ErrorContext::Lookup { .. }
    ));
    // The head fence and expectations come first.
    let fenced = Tx::builder()
        .expect_head(factlite::SnapshotId::new(
            f.store.head().unwrap().store().clone(),
            f.store.head().unwrap().lineage().clone(),
            factlite::Revision::ZERO,
        ))
        .assert(by_key("zzz"), attr("run/status"), key("x"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, fenced).code(), ErrorCode::HeadMismatch);
    let expected = Tx::builder()
        .expect(f.runs[0].clone(), attr("run/status"), vec![key("never")])
        .assert(by_key("zzz"), attr("run/status"), key("x"))
        .build()
        .unwrap();
    assert_eq!(fails(&f, expected).code(), ErrorCode::PreconditionFailed);
}

#[test]
fn retries_compare_the_lookup_not_its_resolution() {
    let f = fixture();
    let keyed = |entity: EntityTerm| {
        Tx::builder()
            .assert(entity, attr("run/status"), key("done"))
            .idempotency_key("finish-a")
            .build()
            .unwrap()
    };
    let request = keyed(by_key("a"));
    let bytes = request.canonical_request();
    // Tag 2, then the attribute as length-prefixed text, then the value.
    let mut tagged = vec![2];
    tagged.extend_from_slice(&7_u64.to_be_bytes());
    tagged.extend_from_slice(b"run/key");
    tagged.push(3);
    tagged.extend_from_slice(&1_u64.to_be_bytes());
    tagged.push(b'a');
    assert!(
        bytes
            .bytes()
            .windows(tagged.len())
            .any(|window| window == tagged)
    );
    let first = f.store.commit(request.clone()).unwrap();
    // Move "a" to run b. An exact retry still returns the original report.
    f.store
        .commit(
            Tx::builder()
                .assert(f.runs[0].clone(), attr("run/key"), key("old-a"))
                .assert(f.runs[1].clone(), attr("run/key"), key("a"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(f.store.commit(request.clone()).unwrap(), first);
    // The ID it resolved to is a different request under the same key.
    assert_eq!(
        f.store
            .commit(keyed(EntityTerm::Id(f.runs[0].clone())))
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    // Reopening replays the keyed lookup against its historical pre-state,
    // and the retry still returns the original report.
    let path = f.path();
    drop(f.store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.commit(request).unwrap(), first);
    // A request with a reference lookup survives the same round trip. Key
    // "a" belongs to run b now; run b's own key moved away, so "b" matches
    // nothing.
    let reference = Tx::builder()
        .assert(f.runs[2].clone(), attr("run/target"), ref_by_key("a"))
        .idempotency_key("target-a")
        .build()
        .unwrap();
    let report = reopened.commit(reference.clone()).unwrap();
    drop(reopened);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.commit(reference).unwrap(), report);
}

#[test]
fn wire_lookups_are_allowed_only_in_changes() {
    let change = |entity: &str, value: &str| {
        format!(
            r#"{{"version":1,"changes":[{{"op":"assert","entity":{entity},"attr":"run/target","value":{value}}}]}}"#
        )
    };
    let lookup = r#"{"lookup":{"attr":"run/key","value":{"string":"a"}}}"#;
    let request = wire::decode_tx(&change(lookup, &format!(r#"{{"ref":{lookup}}}"#))).unwrap();
    assert_eq!(
        request.changes()[0].entity,
        EntityTerm::Lookup(attr("run/key"), key("a"))
    );
    assert_eq!(
        request.changes()[0].value,
        ValueTerm::LookupRef(attr("run/key"), key("a"))
    );
    assert_eq!(
        wire::decode_tx(&wire::encode_tx(&request)).unwrap(),
        request
    );
    let id = r#"{"id":"e1.00000000000000000000000000000001.0000000000000001"}"#;
    let bad = [
        // Malformed lookups.
        change(r#"{"lookup":{"attr":"run/key"}}"#, r#"{"string":"x"}"#),
        change(
            r#"{"lookup":{"attr":"run/key","value":{"string":"a"},"extra":1}}"#,
            r#"{"string":"x"}"#,
        ),
        change(
            r#"{"lookup":{"attr":"run/key","value":"a"}}"#,
            r#"{"string":"x"}"#,
        ),
        change(
            r#"{"lookup":{"attr":"run/key","value":{"string":"a"}},"id":"e1"}"#,
            r#"{"string":"x"}"#,
        ),
        // A lookup value is concrete: no nested lookup or temporary ref.
        change(
            &format!(r#"{{"lookup":{{"attr":"run/target","value":{{"ref":{lookup}}}}}}}"#),
            r#"{"string":"x"}"#,
        ),
        change(
            r#"{"lookup":{"attr":"run/target","value":{"ref":{"temp":"t"}}}}"#,
            r#"{"string":"x"}"#,
        ),
        // Expectations, expectation values and metadata take no lookups.
        format!(r#"{{"version":1,"expect":[{{"entity":{lookup},"attr":"run/key","values":[]}}]}}"#),
        format!(
            r#"{{"version":1,"expect":[{{"entity":{id},"attr":"run/target","values":[{{"ref":{lookup}}}]}}]}}"#
        ),
        format!(r#"{{"version":1,"metadata":{{"m":{{"ref":{lookup}}}}}}}"#),
    ];
    for input in bad {
        assert_eq!(
            wire::decode_tx(&input).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{input}"
        );
    }
    // Query params and standalone values reject lookups too.
    assert!(wire::decode_value(&format!(r#"{{"ref":{lookup}}}"#)).is_err());
}
