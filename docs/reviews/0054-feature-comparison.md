# Review 0054: feature comparison

Branch `docs/feature-comparison`, PR #19. Documentation only. No human reviewer
and no remote CI ran.

## Method

- Four Claude subagents researched two comparators each from primary sources
  (official documentation, repositories, changelogs, and source where the
  documentation is thin), returning each claim with a source URL.
- Self-review: claims that most affect a reader's choice were rechecked against
  the cited pages: Datalevin keeps no history; Datomic documents no HAVING and
  no explicit rule recursion; Datahike as-of, history and purge; Cozo time
  travel. Two cells were reworded as a result: Cozo's timestamps are described
  as application timestamps, not valid time, and Datomic recursion is marked
  not documented.
- An independent Claude subagent fact-checked the draft against the sources
  and the repository, without the research notes.

## Independent fact-check findings

All adopted after confirming the three that change meaning (Datahike history
default in `config.md`, Mentat's pull grammar, the multi-process test names):

- **Mentat pull is flat**, not nested. Fixed.
- **Datahike keeps history by default** (`:keep-history? true`); the footnote
  implied opt-in. Fixed.
- **Datahike writers:** several processes can commit to one branch, with
  fenced commits, by default. Fixed.
- **Cozo writers depend on the engine**, and **Cozo keeps `1` and `1.0`
  distinct as keys** while `==` treats them as equal. Fixed.
- **Mentat as-of, rules and recursion** were checked in source and absent, so
  `no` with a footnote instead of "not documented". Fixed.
- **Prose credited Datomic with recursion** while the table says not
  documented. Reworded.
- **Cells marked `no` without a source** (Datomic HAVING, XTDB rules, XTDB
  lookup refs) changed to "not documented"; Cozo's HAVING equivalent marked as
  application-built.
- **Datahike pod** is labeled beta in its README. Fixed.
- **Prose omissions:** Cozo added to server and distribution; Datahike and
  SQLite added to search. Fixed.
- **Factlite evidence links:** multi-process evidence now points at the CLI and
  pod process tests rather than the in-process thread test; the no-upsert link
  points at `decisions.md` and the pod guide.

The fact-check found every Factlite cell correct and the closing claim (only
Factlite documents keyed retries and a head fence with as-of reads) supported.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally, including the repository
link test. External links are not checked by any test.

## Limitations

- Not every cell was independently rechecked; the fact-check lists Datomic
  `verify-backup`, XTDB `1`/`1.0` equality and backup, and Datalevin type
  coercion and schema migration as unchecked.
- Several cells rely on reading source (DataScript numeric comparison, Mentat
  features) rather than documentation; the notes say so.
- The comparison is dated 2026-11-06 and will go stale.
