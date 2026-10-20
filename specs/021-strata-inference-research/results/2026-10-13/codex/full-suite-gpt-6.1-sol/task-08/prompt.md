Task 8 — Streaming/cancellation bug fix

Inspect `agentix-daemon`'s local chat streaming path. Ensure a client disconnect
cancels the in-flight generation task and releases its owned resources. Add a
focused regression test using the existing streaming test infrastructure; do
not change the SSE wire format.

Acceptance commands:
- `nix develop --command cargo test -p agentix-daemon --test integration local_chat_streaming_returns_sse`
- `nix develop --command cargo clippy -p agentix-daemon -- -D warnings`

Work only in this checkout. Do not commit. Make the requested changes and stop.