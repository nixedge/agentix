# Personal assistant on NixOS

For an existing OurGroceries list, use the standalone `nixos-ourgroceries.nix` alternative and follow [the Rust adapter setup](../../agentix-ourgroceries/README.md). It keeps grocery runtime credentials separate from the explicit choice to require an owner bearer token. Import only one example.

Import `nixos-personal-assistant.nix` on the host running nginx and agentix-agent. It imports the existing Agentix service module itself. The host's flake must provide `inputs` through `specialArgs`, and `inputs.agentix` must include feature025's optional-auth changes and this example.

For example, in the host's module list:

```nix
imports = [
  "${inputs.agentix}/agentix-agent/examples/nixos-personal-assistant.nix"
];
```

The example defaults to **no application token**, relying on your existing WireGuard-only ingress restrictions. Leave Android's optional Owner token blank and explicitly enable the backend. The example does not configure WireGuard, its peer permissions, listener addresses or firewall: your existing HTTPS listener/firewall must limit this route to the peers you authorize. Port8767 remains loopback-only.

To enable bearer authentication, also set:

```nix
services.agentix.agent.environmentFile = "/run/secrets/agentix-agent.env";
```

This example then generates `assistant.token_env = "AGENTIX_ASSISTANT_TOKEN"`, making the secret required at service startup. Have your secret manager populate that root-readable runtime file with:

```text
AGENTIX_ASSISTANT_TOKEN=REPLACE_WITH_A_RANDOM_64_CHARACTER_HEX_TOKEN
```

Enter the actual token in Android's Owner token field. Keep it out of Git, Nix string literals and generated Nix-store files. `/run` secrets must be recreated on boot by your secret manager. A missing/empty/malformed configured token fails startup rather than disabling authentication.

For hand-written TOML: omitted token_env probes AGENTIX_ASSISTANT_TOKEN, using it when present and otherwise accepting requests without a bearer token. Explicit token_env requires its named environment variable. A malformed value is always an error, including when found through the default name.

The generated `/etc/agentix-agent/agent.toml` contains only non-secret settings. Systemd creates the service account and `/var/lib/agentix-agent`. Personal Sled and LanceDB data use separate persistent subdirectories. The required legacy ntfy block is unconfigured; this personal-only example does not provide notification delivery or Google integration. Merge existing legacy database/notification/Google settings if already using those features; do not replace a working legacy configuration blindly.

The example assumes the existing `ai.lan.disasm.us` nginx virtual host already has HTTPS/certificates and WireGuard-only access configured. It adds only `/v1/assistant/`, preserves the full request path and any bearer header, and proxies to127.0.0.1:8767. The inference gateway remains a separate service at `ai.lan.disasm.us:11434`.

After applying the host configuration, inspect `systemctl status agentix-agent` and `journalctl -u agentix-agent`. With no token configured, a valid tell request from an authorized WireGuard peer should reach the application without Authorization; with a token configured, missing/incorrect credentials must return401. The earlier public-route check returned502; deployment and physical phone acceptance remain unperformed. Test grocery add/read/correction and an unchanged retry after a network interruption before changing the default assistant.

## Plaud recording storage

Feature037 adds authenticated recording upload/status/audio/transcript routes under the same assistant prefix. The examples now permit64MiB nginx bodies; instruction JSON is still limited to16KiB by the application. Preserve your actual listener/upstream/TLS/WireGuard settings. Existing deployed16k nginx limits must be updated or recording PUTs will return413 before reaching Rust.

`assistant.recordings_path` stores immutable content-addressed Ogg files beneath `blobs/<first-two-hash-characters>/<sha256>.ogg`. Without this optional setting, the default is `assistant.db_path + ".recordings"`. Recordings, immutable metadata, upload/source deduplication identities, full transcripts and transcription retry state live in the existing personal Sled database. The optional `assistant.recordings_transcription_url` defaults to `gateway_url`; point it at your local Agentix gateway exposing `/v1/audio/transcriptions`, with a Whisper model already loaded. No cloud transcription fallback or automatic model download is introduced.

The Nix agent package supplies FFmpeg. It decodes Opus into bounded30-second WAV chunks and sends them to local Whisper. Successful completion atomically stores the full transcript and deterministic Note chunks with durable LanceDB projection jobs. Upload receipts distinguish original storage from transcription and search readiness. Failed transcription retries after five minutes and never removes original audio.

Back up **personal Sled plus the recordings directory together**, using a consistent stopped-service snapshot or filesystem snapshot. The personal LanceDB index is derived and can be rebuilt. Keep the original server-identity metadata when restoring Sled. Merely copying the index does not back up recordings or transcripts. No garbage collection or automatic deletion is implemented.

On Android, explicitly enable recording uploads on Devices after configuring the existing HTTPS backend. Sync to server works on completed Plaud imports without an awake recorder. Once opted in, recorder sync also triggers upload. Local audio remains available offline. An unresolved request retains its original destination and identity across restart; restore that endpoint to retry after a settings change. No deployment or live phone/server acceptance is implied by the examples.
