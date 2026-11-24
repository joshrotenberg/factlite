# Review 0059: acknowledging MCP subscription streams

Branch `fix/mcp-subscriptions-listen`. A follow-up to plan item 14.3, found
by driving `factlite-mcp` with `mcp-repl` 0.4.0 at the user's request.
Self-review only; no subagent review and no human reviewer ran.

## Finding

In its interactive mode under the `2026-07-28` lifecycle, `mcp-repl` opens a
`subscriptions/listen` stream to keep its command list current. The server
answered -32601, which the client treats as a stream that ended before it
was acknowledged, and it retried with doubling delays for as long as the
session lasted. The one-shot checks recorded in review 0058 do not open the
stream and did not show it.

## Change

`subscriptions/listen` in the `2026-07-28` form is answered with
`notifications/subscriptions/acknowledged`, whose `_meta` names the request
id and whose filter is empty. No response follows; the request stays open
until the client cancels it. Under the handshake form the method is still
-32601. A filter that is not an object is -32602.

The acknowledgment's shape was taken from `tower-mcp`, the library
`mcp-repl` is built on, which implements the revision: the
`SubscriptionsAcknowledgedParams` and `SubscriptionFilter` types and its
`subscription_acknowledgment` helper.

## Self-review

- **Silence after acknowledgment** is required: the revision forbids further
  messages for a cancelled request, and an honored-nothing stream has none to
  send. The test checks that the next line answers the next request and that
  cancelling writes nothing.
- **Ids** are echoed into `_meta` as their raw text, for strings and
  integers, as in responses.
- **Not handled:** a graceful close result for an open stream at end of
  input. The process exits, as for any stdio server.

## Validation

- `crates/factlite-rpc/tests/mcp_stdio.rs`,
  `a_subscription_is_acknowledged_with_nothing_honored_and_stays_silent`.
- `mcp-repl` 0.4.0 with `--protocol 2026-07-28` in interactive mode: one
  warning that auto-refresh is disabled, no retries, and tool calls answered
  as before.
- `REQUIRE_BB=1 ./scripts/check.sh` passes with 307 tests.

## Limitations

- The shape of the acknowledgment rests on `tower-mcp`'s implementation and
  the specification pages read earlier, not on a second independent client.
