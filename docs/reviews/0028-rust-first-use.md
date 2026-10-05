# 7.1 First-use API pass — self-review

Reviewed the complete branch diff, the guide against public signatures and
runtime semantics, and the runnable walkthrough for ownership and cleanup.
No independent review or remote CI ran.

The example uses forward references deliberately, checks both pinned bases,
tests stale condition and head-fence errors, and retains the original request
for retries after later commits and reopen. All source handles close before
reopen; backup is read-only and temporary files clean up automatically.
No public API change was needed. The guide explains final-state batches, exact
types, empty entities, historical schema errors and uncertain commit recovery.

Validation: Cargo offline, local socket permission for assert_cmd; formatting,
Clippy all targets with warnings denied, 137 tests (forty properties at 1,024
cases each), the asserted task workflow, and warning-free rustdoc passed through
`scripts/check.sh`. `git diff --check` passed. This is one application workflow,
not proof that every future application's API requirements are met.
