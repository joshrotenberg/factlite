# Roadmap 9: current-state documentation: reviews

Documentation only.

## Change

- `docs/status.md`: a dated current-state section replaces the drifting
  opening paragraph. The per-item log is unchanged.
- `README.md`: the overview is rewritten for format 4 and the current
  interfaces, and the workflow and CI text is updated.
- `docs/handoff.md`: rewritten around cloning the remote.
- `docs/roadmap.md`: item 9 is marked done.

## Subagent reviews

Two independent subagent reviews ran through kit: Claude (review-pr recipe,
PR #11) and Codex (review-diff profile). They checked every claim against the
code and the other docs. Each finding was confirmed before fixing.

Fixed:

- **A dead link (both).** `docs/decision-briefs/` exists only in unmerged PRs.
  The README link was removed, and the status file and handoff now point to
  the open PRs instead.
- **Item 8 overstated (both).** Only the busy timeout and babashka in CI are
  done; `ANALYZE` and per-process opens remain.
- **The read claim was too broad (Codex).** Transactions and unfiltered history
  read reports, schema reads schema events, aggregates finish in Rust, and very
  large queries use the reference evaluator.
- **"Opens never block writers" overstated (Codex).** Initialization and
  upgrades take the write lock.
- **No status entry or review record for this item (Codex).** Both added, as
  `AGENTS.md` requires.
- **Older log entries contradict the summary (Claude).** The summary now says
  it is current where the log disagrees.
- **The test count needs babashka (Claude).** Stated.
- **The meaning of "through kit" (Claude).** Explained.
- **The handoff's branch claim (Claude).** Softened.
- **The roadmap's RPC crate wording (Claude).** It now says the crate does not
  exist.

Deferred:

- **A documentation link checker (Claude).** It is out of this item's scope,
  so it was added to roadmap item 8 instead.

## Self-review

Checked that format 4, the eight value types, the forty-nine properties, the
CI babashka version and triggers, and the license files match the repository.
No code changed, so no new test run applies. The previous gate (184 tests) is
unaffected.
