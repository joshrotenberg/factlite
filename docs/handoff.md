# Factlite handoff

The full intended local v1 contract and follow-up items 7.1–7.3 are implemented.
Read `AGENTS.md`, `Factlite-Design.md`, `docs/plan.md` and `docs/status.md` before
continuing. The original design remains unchanged. Implementation decisions,
acceptance evidence and recorded item self-reviews are retained in `docs/`.

The last semantic gate passed 142 tests, including forty-two properties at
1,024 cases each, the asserted Rust workflow, Clippy/formatting and warning-free
documentation. The deliberately ignored manual stage profiler passed separately
in release mode. No independent review or remote CI is implied by those results.
Source, Git history, retained local branch names, the lockfile, CLI examples,
benchmark results and this handoff are included in the archive.

## Restore and verify

Keep the archive and its `.sha256` file together. From their directory:

```sh
sha256sum -c factlite-handoff.tar.gz.sha256
tar -xzf factlite-handoff.tar.gz
cd factlite
git status --short
git fsck --full
git log -5 --oneline
git branch --list
```

The archive contains a complete clean checkout with `.git`, not a source-only
`git archive`. The separate `factlite-handoff-manifest.json` records the exact
commit, source tree, branch heads and reachable commit count; the archive's
checksum detects transfer damage. Build outputs, toolchains, scratch databases,
credentials and machine-specific Git configuration are excluded. Do not run
`git init` again or make a replacement initial commit after extraction.

Install Rust with rustup; `rust-toolchain.toml` pins 1.99.0. Then:

```sh
./scripts/check.sh
cargo run --locked -p factlite --example task_workflow
cargo run --locked -p factlite-cli -- --help
```

Cargo may need network access to obtain the locked dependencies on a fresh
machine. The current session's external toolchain/cache is excluded. For this
managed workspace only, source `/workspace/.factlite-tools/env.sh` before Cargo;
normal rustup installations need no such path. `assert_cmd` timeouts require
local Unix socket permission in a restricted execution sandbox.

## Put the restored history in a new remote

Create an empty repository without an initial README, license or `.gitignore`.
Choose visibility deliberately; packages remain unpublished and no license has
been selected. Replace `REMOTE_URL` below with that repository's URL:

```sh
git remote add origin REMOTE_URL
git push --atomic --all origin
git push --tags origin
git ls-remote origin refs/heads/main
git rev-parse main
```

The two main hashes must match. `--all` preserves the item branch names as well
as the main merge history. Do not force-push over an existing unrelated history.
For a remote already populated from this session, clone it normally instead of
uploading the archive again. Verify a fresh remote clone's main hash and Git
integrity before treating the remote handoff as complete. Remote CI may start
when GitHub Actions receives these branches; inspect the main commit's run.

Published 2026-11-05 to the private repository
`https://github.com/joshrotenberg/factlite`. Clone it instead of restoring an
archive. CI runs on pushes to `main` and on pull requests.

## Resume intentionally

There are no known incomplete acceptance rows for the implemented contract.
The API guide is `docs/rust.md`; CLI and backup guides are `docs/cli.md` and
`docs/backup.md`. Continue bounded branch → work → tests → self-review → merge
increments, reporting done, decisions, blockers and what's next after each item.

Since item 9.1, reads and commits load state from a derived, verified datom
interval index; open still replays and validates the full retained log. See
`measurements.md` for observed costs; they are not a production SLA. Existing
reference, stateful, fault, isolation, recovery and backup tests must gate
future optimized paths.

Server/NIF integration, JMESPath, subscriptions and other explicitly deferred
design scope remain deferred. The license and production workload are open
decisions. Continuing a restored database lineage requires retiring the old
writer and retaining identical complete history; divergent writable clones are
unsupported. That database rule is separate from this source/Git handoff.
