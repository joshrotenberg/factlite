# Review: Rust workspace and verification

- **Branch:** `setup/rust-workspace`
- **Scope:** Items 0.3 and 0.4; library/CLI boundaries, toolchain, lockfile,
  shared checks, CI configuration, usage documentation, and status.
- **Reviewer:** Codex self-review; no independent review performed.

## Findings and resolutions

1. Library and binary are both named `factlite`, which caused Cargo's rustdoc
   output-collision warning. Disabled binary rustdoc; library API docs remain
   checked with warnings denied and CLI usage lives in README.
2. The scaffold must not claim working database APIs. The library exposes no
   placeholders, README describes the current state, and CLI accepts only
   `--help`/`--version`. Invalid or unsupported input fails clearly.
3. CLI argument inspection uses OS strings so non-UTF8 input cannot panic before
   it is rejected. Extra arguments must not accidentally execute a recognized
   option. Both paths were smoke-checked.
4. Future CI should execute the local checks, not duplicate their commands.
   The workflow calls `scripts/check.sh`, has read-only repository permissions,
   and pins checkout to a commit. Rustup uses `rust-toolchain.toml` to install
   the exact compiler and components on the runner.
5. Licensing and older-compiler support are not settled. Package publication
   is disabled, no license was invented, and README states the compiler policy.

## Validation

- Reviewed the full staged diff, manifests, generated lockfile, shell script,
  CI configuration, and executable file mode.
- `./scripts/check.sh`: passed without warnings after the rustdoc correction.
  This ran format check, Clippy, tests, and library documentation.
- `cargo build --workspace --locked`: passed.
- CLI smoke checks: correct version, help text, empty stdout/nonzero exit for
  unsupported or malformed invocation, and non-UTF8 rejection.
- `bash -n scripts/check.sh` and `git diff --cached --check`: passed.
- Verified toolchain/minimum version alignment and unpublished package setting.

## Result and limits

No unresolved findings. Ready for local merge. The test runner executed zero
semantic tests; database acceptance evidence begins with step 1. GitHub Actions
is configured but has not run, and no remote repository has been created.
