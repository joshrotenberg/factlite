# Working on Factlite

Read `Factlite-Design.md`, `docs/plan.md`, and `docs/status.md` before changing
semantics. Preserve the supplied design as historical reference; put adopted
implementation decisions in the plan or a focused decision document.

Follow `docs/testing.md`: deterministic unit coverage, semantic property tests,
public integration coverage, and `assert_cmd` scenarios once database CLI
commands exist. Keep rolling from completed items into the next authorized item.

Work on a bounded branch from `main`. Review the complete diff and run checks
appropriate to the change before merging with `--no-ff`. Record review findings
and validation in `docs/reviews/`, including limitations. Self-review must be
labeled as such. Do not imply an independent reviewer or remote CI ran.

At the end of each item, update `docs/status.md` with done, decisions, blockers,
and what's next. Update the plan's checkboxes only when its acceptance criteria
are met. Keep unimplemented behavior explicit.

Keep storage semantics in the library; CLI parsing and rendering belong in the
CLI. Start with one library crate and one CLI crate. Add dependencies when an
implemented item requires them. Use semantic fixtures and property tests for
the core; avoid placeholder tests or tests that merely repeat implementation.

Do not add a server, NIF, generic backend, or other deferred scope. Preserve
typed equality, final-state batch validation, historical schema visibility, and
the distinction between a new no-change commit and an idempotent replay.
