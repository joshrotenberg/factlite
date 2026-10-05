//! Mixed traces use a small domain oracle, independent of batch evaluation and
//! log replay. Logical nodes track slots/sets; expected IDs and reports are
//! calculated before committing, not inferred from returned deltas.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use factlite::{
    AttrName, AttributeDefinition, Cardinality, CommitOutcome, Db, Direction, EntityId, ErrorCode,
    Fact, FactDelta, FactFilter, Operation, Order, Params, Pattern, PulledValue, Query, QueryName,
    Revision, SnapshotId, Store, TempId, Term, Tx, TxReport, Uniqueness, Value, ValueTerm,
    ValueType, wire,
};
use proptest::prelude::*;

fn attr(name: &str) -> AttrName {
    name.parse().unwrap()
}
fn label(name: &str) -> TempId {
    TempId::new(name).unwrap()
}
fn name(name: &str) -> QueryName {
    QueryName::new(name).unwrap()
}
fn definition(name: &str, ty: ValueType, many: bool, unique: bool) -> AttributeDefinition {
    AttributeDefinition::new(
        attr(name),
        ty,
        if many {
            Cardinality::Many
        } else {
            Cardinality::One
        },
        if unique {
            Uniqueness::UniqueOne
        } else {
            Uniqueness::None
        },
    )
    .unwrap()
}

#[derive(Clone, Default, Debug)]
struct Node {
    number: Option<i64>,
    unique: Option<i64>,
    link: Option<usize>,
    tags: BTreeSet<i64>,
}

#[derive(Clone, Debug)]
struct State {
    snapshot: SnapshotId,
    nodes: Vec<Node>,
    schema: BTreeMap<AttrName, AttributeDefinition>,
    extra: BTreeMap<u8, BTreeSet<(usize, u8)>>,
}

impl State {
    fn id(&self, node: usize) -> EntityId {
        let store = self.snapshot.store().to_string();
        format!(
            "e1.{}.{:016x}",
            store.strip_prefix("s1.").unwrap(),
            node + 1
        )
        .parse()
        .unwrap()
    }
    fn facts(&self) -> HashSet<Fact> {
        let mut facts = HashSet::new();
        let mut add = |node, attribute: &str, value| {
            facts.insert(Fact {
                entity: self.id(node),
                attribute: attr(attribute),
                value,
            });
        };
        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(value) = node.number {
                add(index, "trace/number", Value::I64(value));
            }
            if let Some(value) = node.unique {
                add(index, "trace/unique", Value::I64(value));
            }
            if let Some(target) = node.link {
                add(index, "trace/link", Value::Ref(self.id(target)));
            }
            for value in &node.tags {
                add(index, "trace/tags", Value::I64(*value));
            }
        }
        for (slot, values) in &self.extra {
            for (node, value) in values {
                add(*node, &format!("extra/v{slot}"), Value::Bytes(vec![*value]));
            }
        }
        facts
    }
}

fn sort_facts(facts: impl IntoIterator<Item = Fact>) -> Vec<Fact> {
    let mut facts: Vec<_> = facts.into_iter().collect();
    facts.sort_by(|a, b| {
        a.entity
            .cmp(&b.entity)
            .then(a.attribute.cmp(&b.attribute))
            .then_with(|| a.value.compare(&b.value).unwrap())
    });
    facts
}

fn expected_report(before: &State, after: &State, request: &Tx) -> TxReport {
    let old = before.facts();
    let new = after.facts();
    let effective_delta = sort_facts(old.difference(&new).cloned())
        .into_iter()
        .map(|fact| FactDelta {
            operation: Operation::Retract,
            fact,
        })
        .chain(
            sort_facts(new.difference(&old).cloned())
                .into_iter()
                .map(|fact| FactDelta {
                    operation: Operation::Assert,
                    fact,
                }),
        )
        .collect();
    let created_entities: Vec<_> = (before.nodes.len()..after.nodes.len())
        .map(|node| after.id(node))
        .collect();
    TxReport {
        before: before.snapshot.clone(),
        after: after.snapshot.clone(),
        tempids: request
            .new_entities()
            .iter()
            .cloned()
            .zip(created_entities.iter().cloned())
            .collect(),
        created_entities,
        schema_added: after
            .schema
            .iter()
            .filter(|(name, _)| !before.schema.contains_key(*name))
            .map(|(_, definition)| definition.clone())
            .collect(),
        effective_delta,
        metadata: request.metadata().clone(),
    }
}

struct Trace {
    path: std::path::PathBuf,
    store: Option<Store>,
    states: Vec<State>,
    reports: Vec<TxReport>,
    retries: Vec<(Tx, TxReport)>,
}

impl Trace {
    fn store(&self) -> &Store {
        self.store.as_ref().unwrap()
    }
    fn reopen(&mut self) {
        drop(self.store.take());
        self.store = Some(Store::open(&self.path).unwrap());
    }
    fn current(&self) -> &State {
        self.states.last().unwrap()
    }
    fn commit(&mut self, request: Tx, mut next: State, error: Option<ErrorCode>) {
        if let Some(code) = error {
            let failed = self.store().commit(request.clone()).unwrap_err();
            assert_eq!(failed.code(), code, "{request:?}");
            assert_eq!(failed.commit_outcome(), Some(CommitOutcome::RolledBack));
            assert_eq!(self.store().head().unwrap(), self.current().snapshot);
            assert_eq!(
                self.store()
                    .db()
                    .unwrap()
                    .facts(FactFilter::default())
                    .unwrap(),
                sort_facts(self.current().facts())
            );
            // Every rejected operation includes a fresh key. Reuse it with a
            // clean no-op to prove failed schema/allocations/metadata/key did not persist.
            let clean = Tx::builder()
                .idempotency_key(request.idempotency_key().unwrap())
                .metadata("recovered", Value::Bool(true))
                .build()
                .unwrap();
            self.commit(clean, self.current().clone(), None);
            return;
        }
        next.snapshot = SnapshotId::new(
            next.snapshot.store().clone(),
            next.snapshot.lineage().clone(),
            self.current().snapshot.revision().next().unwrap(),
        );
        let expected = expected_report(self.current(), &next, &request);
        assert_eq!(
            self.store().commit(request.clone()).unwrap(),
            expected,
            "{request:?}"
        );
        if request.idempotency_key().is_some() {
            self.retries.push((request, expected.clone()));
        }
        self.reports.push(expected);
        self.states.push(next);
    }

    fn check(&self, basis: usize) {
        let state = &self.states[basis];
        let db = self.store().db_at(&state.snapshot).unwrap();
        assert_eq!(db.snapshot(), &state.snapshot);
        assert_eq!(
            db.facts(FactFilter::default()).unwrap(),
            sort_facts(state.facts())
        );
        assert_eq!(
            db.schema()
                .unwrap()
                .attributes()
                .cloned()
                .collect::<Vec<_>>(),
            state.schema.values().cloned().collect::<Vec<_>>()
        );
        let records = db.transactions(Revision::ZERO).unwrap();
        assert_eq!(
            records.into_iter().map(|r| r.report).collect::<Vec<_>>(),
            self.reports[..basis]
        );
        let events = db.history(FactFilter::default(), Revision::ZERO).unwrap();
        let expected_events: Vec<_> = self.reports[..basis]
            .iter()
            .flat_map(|r| {
                r.effective_delta
                    .iter()
                    .enumerate()
                    .map(move |(ordinal, d)| {
                        (
                            r.after.revision(),
                            ordinal as u64,
                            d.operation,
                            d.fact.clone(),
                        )
                    })
            })
            .collect();
        assert_eq!(
            events
                .into_iter()
                .map(|e| (e.revision, e.ordinal, e.operation, e.fact))
                .collect::<Vec<_>>(),
            expected_events
        );
        // Empty allocations remain known, and IDs allocated later remain absent.
        for index in 0..=self.current().nodes.len() {
            let pulled = db.pull(
                &state.id(index),
                &[attr("trace/number"), attr("trace/link"), attr("trace/tags")],
            );
            if basis == 0 {
                assert_eq!(pulled.unwrap_err().code(), ErrorCode::UnknownAttribute);
                continue;
            }
            let pulled = pulled.unwrap();
            if index >= state.nodes.len() {
                assert!(pulled.is_none());
                continue;
            }
            let pulled = pulled.unwrap();
            let node = &state.nodes[index];
            let mut expected = BTreeMap::new();
            if let Some(n) = node.number {
                expected.insert(attr("trace/number"), PulledValue::One(Value::I64(n)));
            }
            if let Some(n) = node.link {
                expected.insert(
                    attr("trace/link"),
                    PulledValue::One(Value::Ref(state.id(n))),
                );
            }
            if !node.tags.is_empty() {
                expected.insert(
                    attr("trace/tags"),
                    PulledValue::Many(node.tags.iter().map(|n| Value::I64(*n)).collect()),
                );
            }
            assert_eq!(pulled.attributes, expected);
        }
        if basis > 0 {
            self.check_join(&db, state);
        }
        // New attributes cannot leak into an old schema.
        for slot in self.current().extra.keys() {
            let attribute = attr(&format!("extra/v{slot}"));
            if !state.schema.contains_key(&attribute) {
                let error = db
                    .facts(FactFilter {
                        attribute: Some(attribute),
                        ..FactFilter::default()
                    })
                    .unwrap_err();
                assert_eq!(error.code(), ErrorCode::UnknownAttribute);
            }
        }
    }

    fn check_join(&self, db: &Db, state: &State) {
        let query = Query {
            patterns: vec![
                Pattern {
                    entity: Term::Var(name("node")),
                    attribute: attr("trace/link"),
                    value: Term::Var(name("target")),
                },
                Pattern {
                    entity: Term::Var(name("target")),
                    attribute: attr("trace/number"),
                    value: Term::Var(name("number")),
                },
                Pattern {
                    entity: Term::Var(name("node")),
                    attribute: attr("trace/tags"),
                    value: Term::Var(name("tag")),
                },
            ],
            select: vec![name("node"), name("number")],
            order: vec![Order {
                field: name("node"),
                direction: Direction::Asc,
            }],
            ..Query::default()
        };
        let expected: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                if node.tags.is_empty() {
                    return None;
                }
                node.link
                    .and_then(|target| state.nodes[target].number)
                    .map(|number| vec![Value::Ref(state.id(index)), Value::I64(number)])
            })
            .collect();
        assert_eq!(db.query(&query, &Params::new()).unwrap().rows, expected);
    }

    fn run(&mut self, index: usize, (kind, selector, value, flag): (u8, u8, i64, bool)) {
        let mut next = self.current().clone();
        let node = usize::from(selector) % next.nodes.len();
        let other = (node + 1) % next.nodes.len();
        let id = next.id(node);
        let mut tx = Tx::builder()
            .idempotency_key(format!("step-{index}"))
            .metadata("step", Value::I64(index as i64));
        let mut error = None;
        match kind {
            0 => {
                tx = tx.assert(id, attr("trace/number"), Value::I64(value));
                next.nodes[node].number = Some(value);
            }
            1 => {
                if flag {
                    tx = tx.retract(id, attr("trace/tags"), Value::I64(value));
                    next.nodes[node].tags.remove(&value);
                } else {
                    tx = tx
                        .assert(id.clone(), attr("trace/tags"), Value::I64(value))
                        .assert(id, attr("trace/tags"), Value::I64(value));
                    next.nodes[node].tags.insert(value);
                }
            }
            2 => {
                tx = tx
                    .assert(id, attr("trace/unique"), Value::I64(value))
                    .new_entity(label("discarded-on-failure"));
                if next
                    .nodes
                    .iter()
                    .enumerate()
                    .any(|(i, n)| i != node && n.unique == Some(value))
                {
                    error = Some(ErrorCode::UniqueViolation);
                } else {
                    next.nodes[node].unique = Some(value);
                    next.nodes.push(Node::default());
                }
            }
            3 => {
                let owner = next.nodes.iter().position(|n| n.unique == Some(value));
                if let Some(owner) = owner.filter(|owner| *owner != node) {
                    if flag {
                        tx = tx
                            .retract(next.id(owner), attr("trace/unique"), Value::I64(value))
                            .assert(id, attr("trace/unique"), Value::I64(value));
                    } else {
                        tx = tx
                            .assert(id, attr("trace/unique"), Value::I64(value))
                            .retract(next.id(owner), attr("trace/unique"), Value::I64(value));
                    }
                    next.nodes[owner].unique = None;
                } else {
                    tx = tx.assert(id, attr("trace/unique"), Value::I64(value));
                }
                next.nodes[node].unique = Some(value);
            }
            4 => {
                tx = tx.assert(id, attr("trace/link"), Value::Ref(next.id(other)));
                next.nodes[node].link = Some(other);
            }
            5 => {
                let start = next.nodes.len();
                tx = tx
                    .new_entity(label("z-forward"))
                    .new_entity(label("a-cycle"));
                next.nodes.extend([Node::default(), Node::default()]);
                if !flag {
                    tx = tx
                        .assert(
                            label("z-forward"),
                            attr("trace/link"),
                            ValueTerm::TempRef(label("a-cycle")),
                        )
                        .assert(
                            label("a-cycle"),
                            attr("trace/link"),
                            ValueTerm::TempRef(label("z-forward")),
                        )
                        .assert(label("a-cycle"), attr("trace/number"), Value::I64(value));
                    next.nodes[start].link = Some(start + 1);
                    next.nodes[start + 1].link = Some(start);
                    next.nodes[start + 1].number = Some(value);
                }
            }
            6 => {
                let expected = if flag {
                    vec![Value::I64(999)]
                } else {
                    next.nodes[node]
                        .number
                        .into_iter()
                        .flat_map(|n| [Value::I64(n), Value::I64(n)])
                        .collect()
                };
                tx = tx
                    .expect(id.clone(), attr("trace/number"), expected)
                    .assert(id, attr("trace/number"), Value::I64(value));
                if flag {
                    error = Some(ErrorCode::PreconditionFailed);
                } else {
                    next.nodes[node].number = Some(value);
                }
            }
            7 => {
                tx = tx
                    .expect_head(if flag {
                        self.states[0].snapshot.clone()
                    } else {
                        next.snapshot.clone()
                    })
                    .assert(id, attr("trace/number"), Value::I64(value));
                if flag {
                    error = Some(ErrorCode::HeadMismatch);
                } else {
                    next.nodes[node].number = Some(value);
                }
            }
            8 => {
                let slot = selector % 3;
                let attribute = format!("extra/v{slot}");
                let incompatible = flag && next.extra.contains_key(&slot);
                let def = definition(
                    &attribute,
                    if incompatible {
                        ValueType::I64
                    } else {
                        ValueType::Bytes
                    },
                    true,
                    false,
                );
                tx = tx
                    .define_attribute(def.clone())
                    .new_entity(label("schema-empty"));
                if incompatible {
                    error = Some(ErrorCode::ConflictingBatch);
                } else {
                    tx = tx.assert(id, attr(&attribute), Value::Bytes(vec![selector]));
                    next.schema.insert(attr(&attribute), def);
                    next.extra.entry(slot).or_default().insert((node, selector));
                    next.nodes.push(Node::default());
                }
            }
            9 => {
                let (request, report) = &self.retries[usize::from(selector) % self.retries.len()];
                if flag {
                    let mut json: serde_json::Value =
                        serde_json::from_str(&wire::encode_tx(request)).unwrap();
                    json["metadata"]["changed"] = serde_json::json!({"bool":true});
                    assert_eq!(
                        self.store()
                            .commit(wire::decode_tx(&json.to_string()).unwrap())
                            .unwrap_err()
                            .code(),
                        ErrorCode::IdempotencyConflict
                    );
                } else {
                    assert_eq!(
                        self.store()
                            .commit(wire::decode_tx(&wire::encode_tx(request)).unwrap())
                            .unwrap(),
                        *report
                    );
                }
                assert_eq!(self.store().head().unwrap(), next.snapshot);
                return;
            }
            10 => {
                self.reopen();
                assert_eq!(self.store().head().unwrap(), next.snapshot);
                return;
            }
            11 => {
                if flag {
                    self.commit(
                        Tx::builder()
                            .metadata("unkeyed", Value::Bool(true))
                            .build()
                            .unwrap(),
                        next,
                        None,
                    );
                    return;
                }
            }
            12 => {
                tx = tx.retract(id, attr("trace/number"), Value::I64(value));
                if next.nodes[node].number == Some(value) {
                    next.nodes[node].number = None;
                }
            }
            13 => {
                tx = tx
                    .assert(id.clone(), attr("trace/number"), Value::I64(value))
                    .retract(id, attr("trace/number"), Value::I64(value))
                    .new_entity(label("contradiction-empty"));
                error = Some(ErrorCode::ConflictingBatch);
            }
            14 => {
                // A concrete guess at the ID about to be allocated is forbidden;
                // the declared temporary label must be used instead.
                tx = tx.new_entity(label("future")).assert(
                    id,
                    attr("trace/link"),
                    Value::Ref(next.id(next.nodes.len())),
                );
                error = Some(ErrorCode::UnknownEntity);
            }
            _ => {
                self.check(usize::from(selector) % self.states.len());
                return;
            }
        }
        self.commit(tx.build().unwrap(), next, error);
    }
}

fn run_trace(operations: &[(u8, u8, i64, bool)]) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("trace.sqlite");
    let store = Store::open(&path).unwrap();
    let zero = State {
        snapshot: store.head().unwrap(),
        nodes: vec![],
        schema: BTreeMap::new(),
        extra: BTreeMap::new(),
    };
    let mut trace = Trace {
        path,
        store: Some(store),
        states: vec![zero.clone()],
        reports: vec![],
        retries: vec![],
    };
    let mut initial = zero;
    initial.nodes = vec![
        Node {
            number: Some(0),
            unique: Some(0),
            ..Node::default()
        },
        Node::default(),
    ];
    let mut tx = Tx::builder()
        .idempotency_key("initial")
        .new_entity(label("first"))
        .new_entity(label("empty"));
    for def in [
        definition("trace/number", ValueType::I64, false, false),
        definition("trace/unique", ValueType::I64, false, true),
        definition("trace/link", ValueType::Ref, false, false),
        definition("trace/tags", ValueType::I64, true, false),
    ] {
        tx = tx.define_attribute(def.clone());
        initial.schema.insert(def.name().clone(), def);
    }
    tx = tx
        .assert(label("first"), attr("trace/number"), Value::I64(0))
        .assert(label("first"), attr("trace/unique"), Value::I64(0));
    trace.commit(tx.build().unwrap(), initial, None);
    for (index, operation) in operations.iter().enumerate() {
        trace.run(index, *operation);
        trace.check(trace.states.len() - 1);
        trace.check(usize::from(operation.1) % trace.states.len());
    }
    // Complete closure/reopen then verify every historical basis and exact retry.
    trace.reopen();
    for basis in 0..trace.states.len() {
        trace.check(basis);
    }
    for (request, report) in &trace.retries {
        assert_eq!(trace.store().commit(request.clone()).unwrap(), *report);
    }
    assert_eq!(trace.store().head().unwrap(), trace.current().snapshot);
}

#[test]
fn all_commands_interleave_schema_conditions_refs_uniqueness_retry_and_reopen() {
    run_trace(&[
        (1, 0, 1, false),
        (4, 1, 0, false),
        (3, 1, 0, false),
        (2, 0, 0, false),
        (6, 1, 2, false),
        (8, 0, 0, false),
        (5, 0, 1, false),
        (9, 0, 0, false),
        (7, 0, 2, true),
        (8, 0, 0, true),
        (13, 0, 1, false),
        (14, 0, 0, false),
        (10, 0, 0, false),
        (0, 1, 3, false),
        (12, 1, 3, false),
        (11, 0, 0, true),
        (6, 0, 0, true),
        (5, 0, 0, true),
        (9, 1, 0, true),
        (15, 0, 0, false),
    ]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn mixed_trace_matches_independent_domain_state_and_retained_history(
        operations in proptest::collection::vec((0_u8..16, any::<u8>(), -3_i64..4, any::<bool>()), 0..13)
    ) {
        run_trace(&operations);
    }
}
