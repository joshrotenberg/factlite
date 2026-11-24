# Factlite handoff

How to pick this project up from scratch. Read `AGENTS.md`, `Factlite-Design.md`,
`docs/plan.md` and the current-state section of `docs/status.md` first. Open
choices are in `docs/roadmap.md`. Decision briefs awaiting the user are in
their open draft pull requests, listed in `docs/status.md`.

## Get and verify the source

```sh
git clone https://github.com/joshrotenberg/factlite.git
cd factlite
git fsck --full
```

The repository is public. Item branches are kept on the remote alongside
`main`.

Install Rust with rustup; `rust-toolchain.toml` pins 1.99.0. Install babashka
to run the pod's babashka tests, which otherwise skip. CI installs 1.13.229.
Then:

```sh
./scripts/check.sh
cargo run --locked -p factlite --example task_workflow
cargo run --locked -p factlite-cli -- --help
cargo build --release -p factlite-pod
```

Cargo needs network access the first time, to fetch the locked dependencies.
`assert_cmd` timeouts need local Unix socket permission in restricted sandboxes.

## Archive (historical)

A checksummed archive of `main` at `250bc6b` (item 8.2), with all branch heads
and a manifest, was delivered on 2026-11-05. Restoring it gives a full
checkout with `.git`:

```sh
sha256sum -c factlite-handoff.tar.gz.sha256
tar -xzf factlite-handoff.tar.gz
cd factlite && git fsck --full
```

Prefer cloning the remote, which is newer.

## Resume

- **Workflow:** for each item, branch from `main`, open a draft pull request
  with the plan, implement it, and run `scripts/check.sh`. Get independent
  Claude and Codex subagent reviews, record them in `docs/reviews/`, update
  `docs/status.md`, and merge with a merge commit once CI passes.
- **Gates:** the existing reference, stateful, fault, isolation, recovery and
  backup tests must gate any change to storage or reads.
- **Deferred scope:** the NIF, a network listener, subscriptions, JMESPath and
  other explicitly deferred scope need the user's authorization first. The
  JSON-RPC server and its MCP front end were authorized on 2026-11-23 (plan
  step 14).
- **Restoring a store:** continuing a restored database lineage requires
  retiring the old writer and keeping identical complete history. Divergent
  writable clones are unsupported (see `docs/backup.md`).
