# Work status

## 0.1 Inspect and initialize — done

- **Done:** Inspected the empty workspace and Git configuration. Created the
  local repository at `/workspace/factlite` with an initial `main` commit.
  Existing workspace files were left intact. No existing project instructions
  or project checkout were found.
- **Decisions:** Use a project subdirectory rather than the read-only placeholder
  `/workspace/.git`. Local merge commits will preserve each completed increment.
- **Blockers:** None for local Git. Rust is absent; toolchain setup is in progress.
  A remote destination has not been selected.
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
- **What's next:** Merge `setup/plan-and-workflow`, then start the workspace branch.

## 0.3–0.4 Scaffold and verification — pending

- **Done:** Inspected available tooling and began obtaining a Rust toolchain.
- **Decisions:** One library crate and one CLI crate; add dependencies when
  needed. Do not invent working database APIs in the scaffold.
- **Blockers:** Rust installation and verification are not complete yet.
- **What's next:** Branch from the documentation merge, scaffold the workspace,
  establish checks, run them, review, and merge.
