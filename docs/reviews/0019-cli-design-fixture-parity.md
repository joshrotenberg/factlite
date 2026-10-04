# Item 4.4 self-review

Reviewed the complete CLI parser/execution, response encoder, dependency/lockfile,
example, documentation, and subprocess-test diff.

- CLI validates flags and typed input before open, preserves native path encoding,
  requires explicit initialization, and delegates all storage decisions to public
  Store/Db APIs. Read operations capture head once or use a full supplied token.
- Versioned responses preserve tags, decimal numbers, padded bytes, metadata,
  original reports, event ordinals, and flat projection cardinality. Stderr exposes
  stable codes and typed context; exit taxonomy follows the design. Unknown COMMIT
  evidence survives error encoding. Output-write failure does not imply rollback.
- Six assert_cmd scenarios exercise each database command as a separate process,
  complete create/start/A/B/reopen/retry/noop behavior, full Rust report/read parity,
  all six types, all 13 error codes, every exit class, stdin and file input, invalid
  flags/JSON/bounds, spaces/non-UTF8 paths, unknown/empty entities, and no file
  creation for rejected inputs. Each child has a 20-second timeout.
- Initial gate failed because the managed sandbox blocked wait-timeout's Unix
  socket notification write. Rerunning with socket access resolved it; timeout
  checks remain enabled. No production networking was added.
- Formatting, Clippy with denied warnings, 107 tests (33 property suites at 1,024
  cases), documentation with denied warnings, and whitespace passed. The jq/Bash
  documentation tutorial executed successfully in an isolated temporary directory.

No findings remain. Self-review only; remote CI has not run. Joined queries,
physical crash recovery, safe backups, and performance evidence remain subsequent
items. Added assert_cmd/tempfile/test-only SQLite dependencies as needed.
