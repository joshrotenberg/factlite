//! Durable snapshot ownership, historical schema, and invalid-token behavior.

use factlite::{
    AttributeDefinition, Cardinality, Db, ErrorCode, Revision, SnapshotId, Store, Tx, Uniqueness,
    ValueType,
};
use proptest::prelude::*;

fn definition(name: &str) -> AttributeDefinition {
    AttributeDefinition::new(
        name.parse().unwrap(),
        ValueType::String,
        Cardinality::One,
        Uniqueness::None,
    )
    .unwrap()
}

#[test]
fn views_pin_schema_after_commits_dropping_store_and_reopen() {
    fn shareable<T: Clone + Send + Sync>() {}
    shareable::<Store>();
    shareable::<Db>();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("views.sqlite");
    let store = Store::open(&path).unwrap();
    let zero = store.db().unwrap();
    let a = store
        .commit(
            Tx::builder()
                .define_attribute(definition("person/name"))
                .build()
                .unwrap(),
        )
        .unwrap();
    let pinned = store.db().unwrap();
    store
        .commit(
            Tx::builder()
                .define_attribute(definition("task/title"))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(pinned.snapshot(), &a.after);
    assert_eq!(zero.schema().unwrap().attributes().count(), 0);
    assert_eq!(pinned.schema().unwrap().attributes().count(), 1);
    assert!(
        pinned
            .schema()
            .unwrap()
            .get(&"task/title".parse().unwrap())
            .is_none()
    );
    drop(store);
    assert_eq!(pinned.schema().unwrap().attributes().count(), 1);
    let token = pinned.snapshot().clone();
    drop(zero);
    drop(pinned);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened
            .db_at(&token)
            .unwrap()
            .schema()
            .unwrap()
            .attributes()
            .count(),
        1
    );
    assert_eq!(
        reopened
            .db()
            .unwrap()
            .schema()
            .unwrap()
            .attributes()
            .count(),
        2
    );
}

#[test]
fn foreign_store_lineage_and_future_tokens_are_never_clamped() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("a.sqlite")).unwrap();
    let other = Store::open(dir.path().join("b.sqlite")).unwrap();
    let head = store.head().unwrap();
    let foreign = other.head().unwrap();
    let candidates = [
        foreign.clone(),
        SnapshotId::new(
            head.store().clone(),
            foreign.lineage().clone(),
            Revision::ZERO,
        ),
        SnapshotId::new(
            head.store().clone(),
            head.lineage().clone(),
            Revision::new(1),
        ),
    ];
    for token in candidates {
        assert_eq!(
            store.db_at(&token).err().unwrap().code(),
            ErrorCode::InvalidSnapshot
        );
    }
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(store.db_at(&head).unwrap().snapshot(), &head);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn schema_views_keep_their_basis_across_random_commit_counts(count in 0_usize..6){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("property.sqlite");let store=Store::open(&path).unwrap();let mut views=vec![store.db().unwrap()];
        for index in 0..count{store.commit(Tx::builder().define_attribute(definition(&format!("x/attr{index}"))).build().unwrap()).unwrap();views.push(store.db().unwrap());}
        for(index,view)in views.iter().enumerate(){prop_assert_eq!(view.snapshot().revision(),Revision::new(index as u64));prop_assert_eq!(view.schema().unwrap().attributes().count(),index);}
        let tokens:Vec<_>=views.iter().map(|view|view.snapshot().clone()).collect();drop(views);drop(store);let reopened=Store::open(&path).unwrap();
        for(index,token)in tokens.iter().enumerate(){prop_assert_eq!(reopened.db_at(token).unwrap().schema().unwrap().attributes().count(),index);}
    }
}
