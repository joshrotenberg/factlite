//! Public contract checks independent of crate-internal test helpers.

use factlite::{EntityId, ErrorCode, FiniteF64, Revision, SnapshotId, Value};
use std::collections::HashSet;

#[test]
fn public_values_remain_exact_and_foreign_ids_stay_distinct() {
    let local: EntityId = "e1.00000000000000000000000000000001.0000000000000001"
        .parse()
        .unwrap();
    let foreign: EntityId = "e1.00000000000000000000000000000002.0000000000000001"
        .parse()
        .unwrap();
    assert_ne!(local, foreign);
    assert_eq!(
        HashSet::from([Value::Ref(local), Value::Ref(foreign)]).len(),
        2
    );
    assert_eq!(
        Value::F64(FiniteF64::new(-0.0).unwrap()),
        Value::f64(0.0).unwrap()
    );
    assert_eq!(
        Value::I64(i64::MAX)
            .compare(&Value::f64(i64::MAX as f64).unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::TypeMismatch
    );
}

#[test]
fn snapshot_token_preserves_all_bindings_without_claiming_availability() {
    let token =
        "d1.00000000000000000000000000000001.00000000000000000000000000000002.18446744073709551615";
    let snapshot: SnapshotId = token.parse().unwrap();
    assert_eq!(snapshot.version(), 1);
    assert_eq!(snapshot.revision(), Revision::new(u64::MAX));
    assert_eq!(snapshot.to_string(), token);
}
