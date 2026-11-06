# Item 8: CLI busy timeout and babashka in CI: reviews

## Change

- The CLI accepts `--busy-timeout-ms MS` on `init` and every database command.
  It is parsed and validated in `parse` through `StoreOptions`, and passed to
  writable opens, read-only opens and reads at a snapshot
  (`Db::open_read_only_with(..).at(..)`, which calls the same library functions
  as `open_read_only_at`).
- CI installs babashka 1.13.229. The download is checked against the SHA-256
  published with that release, extracted into its own directory on `PATH`, and
  confirmed with `bb --version`. CI then sets `REQUIRE_BB` so the pod's
  babashka tests run.

## Subagent reviews

Two independent subagent reviews ran through kit: Claude (review-pr recipe,
PR #10) and Codex (review-diff profile). Findings were checked against the code.

Fixed:

- **A flaky timing bound (both).** The test required a 150 ms timeout to finish
  in under 1.8 s, including process startup. It now compares a 0 ms and a
  1,500 ms run: the long run must take at least 1,500 ms, and must take 800 ms
  longer than the short one. No absolute upper bound remains.
- **Untested open paths (Claude).** Added `backup`, `transactions --at`, a
  foreign `--at` token (still `InvalidSnapshot`), and `init` against a held
  lock, which must fail `Busy`.
- **Late validation (Claude).** The timeout is now validated in `parse`, so an
  invalid value fails before stdin is read. Tested.
- **`init` errors (Claude).** Other `init` shapes now fail with
  `init expects PATH [--busy-timeout-ms MS]`. Tested.
- **Overflow branch untested (Claude).** Added `99999999999999999999`.
- **The whole `$RUNNER_TEMP` on `PATH` (Claude).** babashka now extracts into
  `$RUNNER_TEMP/bb`, and only that directory goes on `PATH`.
- **Fragile download (Claude).** Added `curl --retry 3`, a `bb --version` step,
  and a comment naming the source of the checksum.
- **Overstated docs (Claude).** The CLI docs no longer say reads never wait.

Rejected:

- **Leading zeros accepted (Claude).** This is consistent with the wire
  format's decimal parsing, and the value stays bounded by `StoreOptions`.
- **`REQUIRE_BB` set for the whole `check.sh` (Claude, "probably intended").**
  Only the pod's babashka tests read it.

Not verified by either reviewer: both had no download access. The checksum was
taken from the release's published `.sha256` asset, and the CI run on this PR
is the check that it matches and that the archive layout is right.

## Self-review and validation

Self-review: confirmed that the four open paths in `run` all receive the parsed
options, and that `.at` reproduces `open_read_only_at` exactly. Validation:
`REQUIRE_BB=1 scripts/check.sh` passed locally with 184 tests.
