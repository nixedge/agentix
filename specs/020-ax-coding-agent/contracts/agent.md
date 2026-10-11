# Agent and CLI contract v1

## CLI and commands

`ax` opens the TUI; `ax exec [--json] [--model ID] [--approval on-request|never] PROMPT` runs headless. Existing `ax PROMPT` and `ax -` retain one-shot/stdin behavior. `ax resume [ID|--last]`, `ax fork ID` and `ax sessions` operate on local session metadata. All agent/tool execution must be in ax-jail; direct unjailed execution refuses with launch guidance (help/version and read-only session listing are allowed).

Options preserve `--gateway`, `--cwd`, `--no-shell`, `--no-cloud` and `--max-calls`. New flags configure session/model/approval policy without granting additional filesystem roots. Config precedence: explicit CLI, environment allowlist, user `~/.ax/config.toml`, built-in defaults; project config can suggest tools/settings but cannot authorize executable servers or permissions. Invalid configuration fails with source/key diagnostics and redacted values.

Slash commands: help, model, status, new, resume, fork, compact, diff, plan, permissions, skills, mcp, agents, quit. Dispatch locally; unknown names return usage. `/model` has Local and Hugging Face views. `/agents` lists task, worker, model, worktree, lease/progress and action controls. `/plan` toggles read/search/diff-only tools at the next turn boundary. Model, mode, resume and fork changes during a turn are explicitly deferred until idle or rejected with a clear reason; never change in-flight execution.

Ctrl-C during work requests cancellation; a second Ctrl-C forces frontend exit after recording interrupted state. Idle Ctrl-C exits. Restore raw/alternate-screen state through a drop guard on errors/panic paths. Resize, Unicode cursor movement, bracketed paste, multiline input and scrollback must be tested.

## Events

Headless stdout in JSON mode consists only of JSONL objects:

`{schema_version:1, session_id, turn_id, sequence, type, payload}`

Types: session_started, turn_started, text_delta, tool_started, tool_output, tool_result, approval_required, model_operation, model_changed, context_compacted, worker_status, turn_completed, turn_failed, turn_cancelled. Diagnostic logs go to stderr. Invocation IDs correlate tool events; byte truncation is explicit. A terminal event is emitted once when the frontend can finish normally. A process crash may end the stream without one; resume records interruption.

Exit codes: 0 completed; 1 runtime/tool-task failure; 2 usage/config; 3 approval required in headless mode; 130 cancelled. Successful command execution is reported using its actual exit code and does not by itself imply task completion. Pending approvals never block headless mode waiting on nonexistent input.

## Local inference and escalation

Use existing `/v1/chat/completions` with typed tool definitions/results and SSE. The daemon validates the selected opaque local model ID and forces local routing even when a display alias resembles `provider/model`. Streaming response deltas distinguish text and tool-call arguments; finish_reason=tool_calls terminates a complete tool batch. On truncated/error streams, no partial calls execute. Duplicate IDs, invalid JSON or unregistered names are errors with bounded repair attempts.

The only public cloud inference entry point is `POST /v1/agentix/escalations`. Its EscalationRequest contains knowledge, question, local_insufficiency, selected cloud target and invocation context. All three rationale fields must be nonempty and bounded. The daemon checks configured escalation permissions; caller-supplied approval text cannot override those permissions. agentix-router validates the payload and creates a non-deserializable ValidatedEscalation with private fields and no unchecked public constructor. Every internal provider executor requires this type; no generic chat/proxy executor can select a cloud backend without it. Add compile-fail coverage for attempts to construct/bypass the validated type and runtime tests that denied requests cause zero provider HTTP calls.

Existing `/v1/chat/completions`, `/v1/responses`, `/v1/messages`, embeddings and Ollama-compatible routes remain local-only. Resolve registered local IDs/aliases first. Any request otherwise selecting Anthropic, OpenAI or OpenRouter returns HTTP 403 with code `structured_escalation_required` and migration guidance, without provider traffic. The daemon does not invent a rationale, silently remap a cloud model to local, or offer a configuration switch restoring the bypass. Unsupported local capabilities retain their explicit unsupported error.

This intentionally breaks previous direct-cloud compatibility. Bump agentix-api and agentix-daemon from 0.1.0 to 0.2.0, update internal dependents/lockfile, and publish local migration documentation as part of implementation. Existing cloud clients must supply the typed escalation payload to the new endpoint; local-compatible request/response shapes stay unchanged. No constitution exemption is needed.

## Built-in tools and approvals

Read/list/search take bounded paths/results; apply_patch takes exact patch text and expected file hashes; run_command takes command, cwd, timeout and PTY preference; inspect_diff is read-only. All relative paths resolve against session cwd and stay within grants. Upstream filename search follows symlinks unchanged, as explicitly requested during implementation; its starting root is not a containment boundary, and the jail controls accessible mounts. Reads and edits validate canonical paths and protect against symlink races at execution, not only at preview. All patch targets are checked before writing; if a conflict occurs, report it and preserve the user's prior content. A multi-file I/O failure reports exactly which writes completed; no false all-or-nothing claim.

Approval binds request/turn/tool/cwd/arguments and proposed effects. Denial returns a tool result; session-level allow rules remain limited to the launch grants. Command output can contain hostile instructions/escape sequences and is never interpreted as control events. MCP tools get the same approval and output policies. No user instruction, skill or model output can grant a host mount or host-shell execution.
