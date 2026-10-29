# Planned assistant boundary (increment C; not implemented by A)

The application's logical entry is tell(text), with request UUID, conversation UUID and explicit time context supplied in the request envelope. Android's existing TellCommand remains a client-side seam until the actual server contract is implemented and tested.

The server returns a response with request/conversation IDs, reply text and structured outcomes that distinguish saved, completed, pending, clarification and failure. A timeout can have an uncertain outcome; the client reuses the request ID. Tool operations derive stable per-command execution identities, so repeated generation or HTTP retry does not duplicate writes.

Reuse the existing agentix-agent listener for the later versioned HTTP endpoint, reachable over the owner's WireGuard network. Do not introduce a public MCP listener to satisfy the Android path. A future MCP adapter invokes the same application operations using a compliant MCP transport; it does not bypass validation or write directly to either database.

Inference interface consumes recent conversation and retrieved context and produces typed proposed tool calls or a reply. Provider selection remains outside domain/storage; generation and embedding profiles are independently configurable. The baseline is agentix-llama; a future agentix-qwen adapter implements the same contract.
