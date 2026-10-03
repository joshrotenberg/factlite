# Work status

Repository setup and item 1.1 are complete. The next increment is item 1.2,
schema and allocation state. Database persistence is not implemented yet.
Reviews are self-reviews; no independent reviewer or remote CI has run.

## 0.1 Inspect and initialize — done

- **Done:** Inspected the empty workspace and Git configuration. Created the
  local repository at `/workspace/factlite` with an initial `main` commit.
  Existing workspace files were left intact. No existing project instructions
  or project checkout were found.
- **Decisions:** Use a project subdirectory rather than the read-only placeholder
  `/workspace/.git`. Local merge commits will preserve each completed increment.
- **Blockers:** None for local Git. Rust was absent at inspection and is now
  installed under item 0.3. A remote destination has not been selected.
- **What's next:** Record the contract, roadmap, and workflow on
  `setup/plan-and-workflow`.

## 0.2 Record the contract and roadmap — done

- **Done:** Preserved the handoff unchanged and wrote the README, contributor
  workflow, and detailed plan with acceptance criteria. Completed a self-review,
  checked local documentation links and whitespace, and verified the original
  design is byte-identical. Review: `reviews/0001-plan-and-workflow.md`.
- **Decisions:** Adopt the design's recommended v1 semantics as the working
  contract. Use replay first and defer current projections to measured need.
  Keep each increment small enough for a complete diff review.
- **Blockers:** None.
- **What's next:** Documentation branch merged; workspace setup follows in
  items 0.3 and 0.4.

## 0.3 Scaffold the workspace — done

- **Done:** Installed Rust 1.99.0, Clippy, and rustfmt. Created the `factlite`
  library and `factlite-cli` package with a `factlite` binary, checked-in
  lockfile, workspace lints, pinned toolchain, and build instructions. CLI help
  and version work; unsupported commands exit 2 with stderr and empty stdout.
- **Decisions:** One library crate and one CLI crate; add dependencies when
  needed. Edition 2024, current stable Rust 1.99.0, and no older-compiler promise.
  All packages are unpublished. CLI usage is documented in README; binary
  rustdoc is disabled to avoid a name collision with the library documentation.
- **Blockers:** None. License and remote destination are open decisions, not
  blockers to local development.
- **What's next:** Begin typed values and identity on a new semantic-core branch.

## 0.4 Establish verification — done

- **Done:** Added executable `scripts/check.sh` and GitHub Actions configuration
  invoking the same script. Formatting, Clippy with denied warnings, the test
  runner, and library documentation passed locally. Built both packages and
  smoke-checked CLI help/version, unsupported commands, extra arguments, and
  non-UTF8 input. Checked shell syntax, staged whitespace, and package settings.
  Review: `reviews/0002-rust-workspace.md`.
- **Decisions:** Use one shared verification entry point. GitHub Actions has
  read-only contents permissions and checkout pinned to a commit. There are
  zero semantic tests at this stage; checks prove the scaffold builds only.
- **Blockers:** None locally. Remote CI is configured but cannot run until
  the repository has a remote destination.
- **What's next:** Add meaningful typed-value and identity acceptance tests with
  item 1.1, then review and merge that increment separately.

## 1.1 Typed values and identity — done

- **Done:** Six exact value types, finite-float validation/normalization,
  typed equality/hash, same-type comparisons, strict store/entity/lineage/
  snapshot tokens, revisions, and temporary labels. Eleven tests passed,
  including seven property suites with 1,024 cases each and two public
  integration tests. Format, Clippy, tests, and docs passed.
- **Decisions:** `FiniteF64` makes invalid floats unrepresentable through safe
  constructors; mixed-type ordering errors instead of coercing. Parsing IDs
  validates syntax only; item 1.2 supplies allocation membership. Token encodings
  and testing policy are recorded in `decisions.md` and `testing.md`.
- **Blockers:** None locally. No remote is needed for the workflow.
- **What's next:** Item 1.2, immutable schema and revision-aware allocation.

## Environment notes

The local toolchain lives outside Git in `/workspace/.factlite-tools`. For this
environment, source `/workspace/.factlite-tools/env.sh` before running Cargo.
Standard development machines can use their usual rustup installation.

## Open repository decisions

- Remote hosting destination: none selected; branch/review/merge is local.
- License: none selected; publishing is disabled.
