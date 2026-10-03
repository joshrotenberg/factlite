# Review: item 2.2, authoritative log and replay

- **Branch:** `storage/log-and-replay`
- **Reviewer:** Codex self-review.

Reviewed full codecs, report/event persistence, bounded replay, state validation,
open integration, tests, and staged diff. Reports/events are mutually checked;
state reconstruction validates identities, schemas, effective changes, and
constraints rather than trusting an encoded report. All dependent reads use the
caller's existing SQLite transaction. Historical upper bounds never load later
reports. Numeric blobs preserve signed/finite ordering without affinity coercion.

Review/test corrections:

- Reader counts are bounded by remaining input before reading or iterating.
- Noncanonical zero, nonfinite floats, malformed tags/UTF-8, trailing bytes,
  duplicate metadata/temporary labels, and truncated structured reports reject.
- The first corruption fixture tried deleting a referenced entity and SQLite
  correctly blocked it. The fixture now explicitly disables FK checks for that
  artificial mutation; production FK checks remain enabled and reopen catches it.
- Allocation reconstruction uses identity order to restore submitted labels, so
  alphabetical report-map order cannot change original ID allocation.

`scripts/check.sh` passed: fifty-six tests, twenty-two 1,024-case properties,
randomized SQLite/replay/reference traces, corruption/reopen scenarios, public
integration suites, formatting, Clippy, and docs. No unresolved findings. Public
atomic commits and their failure outcomes are item 2.3; retries remain step 4.
