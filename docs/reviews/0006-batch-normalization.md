# Review: item 1.4, pure batch normalization

- **Branch:** `core/batch-normalization`
- **Reviewer:** Codex self-review.

Reviewed the full normalizer, public fact/delta types, tests, and staged diff.
Request order does not execute sequentially: assertions/retractions are resolved
into sets, conflicts checked together, replacements calculated together, and
effective changes come from state differences. Input state is never mutated.

Added a successful disjoint-domain property because arbitrary overlapping
add/remove sets mostly exercise conflicts. It independently checks the set
equation and reapplies effective events to recover final state. Other properties
cover order reversal and cardinality-one conflicts. Regression fixtures cover
cycles, absent retractions, type errors, retained empty identities, explicit old
retractions, and guessed future IDs in both entity and reference positions.

During integration, removing the catalog lint expectation exposed an unused
schema projection path. Normalization now resolves a schema at its staged basis,
using the same revision-aware schema abstraction as later reads.

`scripts/check.sh` passed: twenty-nine tests, thirteen 1,024-case properties,
public integration suites, formatting, denied-warning Clippy, and docs. No
unresolved findings. Preconditions/uniqueness are item 1.5; normalization remains
private and does not expose partial commits or reports.
