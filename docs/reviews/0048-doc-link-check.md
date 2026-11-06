# Roadmap 8: documentation link check: reviews

## Change

`crates/factlite/tests/doc_links.rs` is an integration test, part of
`cargo test`, so `scripts/check.sh` and CI run it.

- It walks the repository's Markdown without following symbolic links,
  skipping `target` and hidden directories.
- It parses each file with pulldown-cmark, a test-only dependency.
- It requires every link and image target other than `http`, `https`,
  `mailto` and same-page anchors to resolve, with exact case, to an existing
  path inside the repository.

## Subagent reviews

Two independent subagent reviews of the first version ran through kit: Claude
(review-pr recipe, PR #12) and Codex (review-diff profile). Both found that the
hand-written parsing was unsafe. Findings were confirmed with fixtures before
fixing.

Fixed by switching to pulldown-cmark:

- **Fences and code spans could hide links or invent them (both).** A fence
  that never closed made the checker skip the rest of the file without saying
  so.
- **Targets were truncated (both).** Paths with parentheses and angle-bracket
  destinations with spaces were cut short.
- **Reference links were not checked (both).**

Fixed in the path rules:

- **Absolute targets were checked against the host filesystem (both).** They
  now resolve from the repository root, as GitHub does.
- **Targets that leave the repository were accepted (Claude).** They now fail.
- **Symlinks were followed (both).** The walk no longer follows them, so a
  cycle cannot recurse.
- **Unstable order (Codex).** Files are sorted, so failures appear in a stable
  order.
- **Case-insensitive existence on macOS and Windows (Claude).** Each path
  component is now compared exactly.
- **`file:42` was taken for a URL scheme and skipped (Codex).** Only `http`,
  `https` and `mailto` count as external.
- **Unstripped and undecoded targets (Claude).** Query strings and `:line-line`
  suffixes are stripped, and percent-encoded paths are decoded.
- **An empty path passed (Claude).** It now fails.
- **A non-UTF-8 file panicked (Claude).** It is now reported.
- **The checker could go blind unnoticed (Claude).** The main test requires at
  least 50 checked links; there are more than 80 today.
- **Thin tests and no negative test (Claude).** A fixture test now covers every
  rule, with exact broken-link output and counts.

Accepted as limits:

- Raw HTML links and anchor targets are not checked. This is documented.
- The repository root is found two directories above the crate. The crate is
  not published, so a packaged copy is not a concern.

## Self-review and validation

Self-review: the fixture's expected output lists exactly the five planted
failures, in document order. `REQUIRE_BB=1 scripts/check.sh` passed locally
with 186 tests.
