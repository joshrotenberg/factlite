# Review 0060: MCP argument error messages

Branch `fix/mcp-argument-messages`. A follow-up to plan item 14.3, found in an
interactive `mcp-repl` session. Self-review only; no subagent review and no
human reviewer ran.

## Finding

`tool transact {}` returned a tool error whose message read
`invalid params: missing member: tx`. The prefix is the JSON-RPC server's
wording for a -32602 protocol error. In MCP the same failure is a tool result
for the model, where the prefix says nothing useful.

## Change

`failure_json` in `crates/factlite-rpc/src/mcp.rs` uses the dispatcher's own
message for an argument failure. Factlite errors, which carry their own error
object, are unchanged. The JSON-RPC server's -32602 messages are unchanged.

## Validation

- `argument_errors_name_the_problem_without_protocol_wording` in
  `crates/factlite-rpc/tests/mcp_stdio.rs` compares five argument errors, and
  a repeated argument, with the exact expected object. The prefix restored
  fails it.
- `REQUIRE_BB=1 ./scripts/check.sh` passes with 308 tests.

## Limitations

None beyond those of review 0058.
