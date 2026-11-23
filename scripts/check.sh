#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --quiet --locked -p factlite --example task_workflow
cargo run --quiet --locked -p factlite --example work_queue >/dev/null
cargo run --quiet --locked -p factlite --example agent_session >/dev/null
RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings" cargo doc --workspace --no-deps --locked
