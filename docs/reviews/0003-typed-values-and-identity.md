# Review: item 1.1, typed values and identity

- **Branch:** `core/typed-values-and-identity`
- **Reviewer:** Codex self-review.
- **Scope:** Primitive types, error classifications, token encodings, and tests.

## Findings and resolutions

- Float payloads must not bypass finite/zero validation. The public enum carries
  a private validated wrapper; both construction paths share its invariant.
- Mixed-type ordering cannot silently impose a tag order. `Value::compare`
  returns structured `TypeMismatch`; no public mixed-value `Ord` is exposed.
- Token parsing cannot establish membership. Public docs and decision notes
  explicitly require later catalog validation. Store binding participates in
  equality, hashing, and reference ordering.
- Integration test crates inherit documentation warnings; added a crate comment
  after the initial lint failure. Finite-order properties use a finite generator;
  separate raw-bit properties exercise rejection at the float boundary.
- Randomized tests are coverage, not exhaustive proof; the testing policy says
  this explicitly and specifies scenario coverage for future CLI commands.

## Validation and result

`scripts/check.sh` passed: format, denied-warning Clippy, unit/property/integration
tests, and documentation. Eleven tests, including seven properties at 1,024 cases
each. Reviewed source, tests, lockfile, decision notes, and staged whitespace.
No unresolved findings. Membership and historical availability remain item 1.2
and the later store/view work; no persistence or CLI transaction claims are made.
