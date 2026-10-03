# Review: plan and workflow

- **Branch:** `setup/plan-and-workflow`
- **Scope:** Items 0.1 and 0.2; local repository, original design, implementation
  roadmap, contributor workflow, and item status.
- **Reviewer:** Codex self-review; no independent review performed.

## Findings and resolutions

1. The handoff says implementation needs separate authorization. The user's
   current instruction supplies that authorization; README records it while
   leaving the original handoff unchanged.
2. The roadmap must preserve the full contract rather than narrow it to a day.
   All design phases remain represented with explicit acceptance criteria.
3. Deferring retry implementation could allow callers to submit keys without
   protection. Step 2.3 now explicitly rejects keyed requests until retry support
   is implemented. Step 4 requires lookup before current-state validation.
4. Metadata restrictions and stable structured errors deserve explicit items.
   These were added to the semantic-core acceptance criteria.
5. Historical query compilation cannot assume current tables. Step 5.4 calls
   for a deliberate historical strategy checked against reference evaluation.

## Validation

- Reviewed the full staged documentation diff and original handoff.
- `git diff --cached --check`: passed.
- Verified repository-relative Markdown links in the new documentation.
- Original design preserved byte-for-byte; SHA256:
  `3196dfbc73906c573737800a5457896a541a9f5e1919aa3fcbae6c6a00af7ae2`.

## Result

No unresolved findings. Ready for a local merge. No implementation or semantic
tests exist in this increment; this review establishes the documented baseline.
