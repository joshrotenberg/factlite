# JSON-RPC interface design: reviews

Design documentation only; no code changed.

## Subagent reviews

Two independent subagent reviews of the first draft ran through kit: Claude
(review-pr recipe, on PR #4) and Codex (review-diff profile, on the branch).
They had the diff and the code, not the author's conclusions. Findings were
checked against the code and the JSON-RPC 2.0 specification before acting.

Confirmed and fixed (both reviewers unless noted):

- Replying -32600 to a notification `transact` violates the specification,
  which forbids any response to a notification. Now it is not executed, gets no
  response, and is logged.
- Rejecting a whole batch containing `transact` conflicts with per-entry batch
  responses. Now order-dependent entries (`transact`, `open`, `close`,
  `backup`, `shutdown`) get their own error, and the others are processed.
  Claude noted the other order-dependent methods.
- Read requests have no public wire envelopes, and `wire::strict` is private;
  the CLI parses read options itself. The draft overstated "existing
  envelopes". Read params are now defined, and public read decoders are a
  prerequisite.
- `wire::encode_error` nests the error object under `"error"`. `error.data` is
  now specified as the inner object.
- Promised parallel reads conflict with the single `Arc<Mutex<Connection>>`
  that `Store` and `Db` share. The design now states that requests on a handle
  serialize, and leaves extra read connections as a later option.
- Adding `basis` to wire read outputs would change the version-1 wire outputs
  (Claude). Reads now return `{basis, value}`, leaving the wire outputs
  unchanged.
- `open` would create missing files and left read-only handles, duplicate opens
  and `close` undefined (Claude). `open` now needs `create: true` to create, and
  the other cases are specified. The handle field is renamed from `store`,
  which collided with snapshot tokens.
- Backup copies preserve identities, so writable opens of a source and its copy
  diverge (Codex). This is now stated, with a pointer to `docs/backup.md`.
- Missing hazards (Claude): trust of client-supplied paths, framing limits,
  unbounded results, unkeyed `Unknown` outcomes, and SIGKILL during shutdown.
  All are now covered.
- The build trigger's "or a consumer is ready" contradicted its rationale, and
  "later items only add methods" was wrong for items 5 and 6. Fixed in the
  design and the roadmap.

Rejected: none.

## Self-review

Self-review of the revision: checked that every method in the table maps to an
existing `Store` or `Db` operation, that all JSON-RPC codes used are the
standard reserved ones or -32000 in the server range, and that the roadmap,
`AGENTS.md` and `decisions.md` agree that this is designed but not authorized.
