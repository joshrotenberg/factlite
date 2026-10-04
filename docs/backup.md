# Backups and lineage

Use `Store::backup(destination)` or:

```sh
factlite --db demo.sqlite backup --to backup.sqlite > backup.head.json
factlite --db backup.sqlite query --file examples/query.json --at-file A.json
factlite --db backup.sqlite transactions
```

Backup uses SQLite's online backup API inside one fixed source read snapshot,
including committed WAL content. It returns the full snapshot token actually
copied. Other connections may commit while it runs; those later commits need not
be in this backup. A newer token then fails against the backup rather than being
silently clamped. Copying only a live main database file is not this mechanism.

The destination must be new, with no existing main file or WAL/SHM/journal sidecar.
A private staging file in the same directory is copied, validated against the
captured token and complete retained log, converted to DELETE journal mode,
closed and synced before no-replace publication. Unix also syncs the directory.
Existing paths, including a path created during copying, are never overwritten.
Staging is cleaned on ordinary failures. Publication uses tempfile's portable
no-clobber primitive; no stronger cross-filesystem/platform atomicity or physical
power-loss claim is made. A directory-sync failure can return an error after the
complete destination has appeared; inspect it through read-only access.

The result is a standalone file with all retained revisions, schema, allocations,
metadata, reports, retry evidence and original store/lineage identity. Saved
tokens still work. Rust uses `Db::open_read_only(path)` or
`Db::open_read_only_at(path, &token)`; these use SQLite read-only connections and
cannot initialize files or expose commits. CLI read commands use the same path.
Temporary query relations remain connection-local. Reads of a standalone backup
do not change its bytes or create WAL sidecars.

A read-only backup can coexist with its active source. To resume writes after a
disaster, retire every previous writer and ensure the recovered copy has
**identical complete retained committed history**, including retry records and
reports. The restore test closes all old handles, removes the retired source,
reopens the faithful backup with Store, verifies original tokens/retries, then
continues at the next revision.

If later committed history is missing from an older backup, continuing writes
under the same lineage is unsupported. Divergent writable clones, relineaging,
branching and merging are outside v1. Arbitrary copying cannot be detected
reliably: callers must uphold writer retirement and history identity. No API or
test claims automatic clone detection. A snapshot token alone is not a backup.
