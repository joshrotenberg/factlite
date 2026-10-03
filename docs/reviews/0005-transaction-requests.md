# Review: item 1.3, owned transaction requests

- **Branch:** `core/transaction-requests`
- **Reviewer:** Codex self-review.

Reviewed the complete source, tests, and diff. Builder/request fields are owned;
no database handle or cursor is retained. Private request fields preserve
validation after build. Structural checks are independent of current schema and
identity membership, preserving the later retry-before-state-validation contract.
Metadata rejects references and expectations normalize typed sets without
changing operation order. Internal tag ordering is limited to deterministic
encoding; public mixed-type ordering continues to return an error.

Tightened the property test to verify that duplicate and ordered operations
survive building while expected values normalize as sets. Forward references,
cycles, duplicate declarations, undeclared labels, invalid metadata, and the
intentionally deferred batch-conflict check have deterministic coverage.

`scripts/check.sh` passed: twenty-two tests including ten 1,024-case properties,
public integration checks, formatting, Clippy, and docs. No unresolved findings.
Wire decoding, canonical retries, and commit semantics are intentionally absent
at this item; the builder describes requests but does not commit them.
