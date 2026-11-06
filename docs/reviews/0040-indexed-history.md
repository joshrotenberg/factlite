# 11.3 Indexed filtered history: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Db::history` dispatches to `history_from_index` when the filter names an
entity or attribute, and to `history_from_log` otherwise. The index path is one
SQL `UNION ALL` of assertion and retraction endpoints joined to `fact_events`
on revision, operation and fact, ordered by revision and ordinal.

Self-review checked these points:

- Each effective event is exactly one interval endpoint. Open verifies that
  intervals equal intervals derived from fact events, so the join finds
  exactly one event per endpoint. Each event row carries its original ordinal.
- Bounds are exclusive below and inclusive above, matching `reports_between`.
  The lower bound is checked before reading, as before.
- Filter validation against the view's schema is unchanged on both paths.

## Evidence

- New 1,024-case property: interval history equals log history across views,
  lower bounds and filter shapes, including never-changed pairs and
  since-retracted values.
- Existing history, revision-read, isolation, CLI and pod tests pass on the new
  path, and the pod's lookup-ref history check runs through it.
- Measurement: see `measurements.md`.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 173
tests, the Rust workflow and warning-free rustdoc.

## Limitations

- Attribute-only history still visits every interval of the attribute.
- Unfiltered and value-only history read every report in the range.
