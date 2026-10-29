# Personal assistant on NixOS

Import `nixos-personal-assistant.nix` on the host running nginx and agentix-agent. It imports the existing Agentix service module itself. The host's flake must provide `inputs` through `specialArgs`, and `inputs.agentix` must include feature025's conversational/Android changes and this example.

For example, in the host's module list:

```nix
imports = [
  "${inputs.agentix}/agentix-agent/examples/nixos-personal-assistant.nix"
];
```

Have your secret manager populate `/run/secrets/agentix-agent.env`, readable by root, with one line:

```text
AGENTIX_ASSISTANT_TOKEN=REPLACE_WITH_A_RANDOM_64_CHARACTER_HEX_TOKEN
```

Use the actual randomly generated token as the Android app's owner token. Keep it out of Git, Nix string literals and generated Nix-store files. Change `environmentFile` in the example if your secret manager uses another path. The service requires the file to exist before startup. `/run` secrets must be recreated on boot by your secret manager.

The generated `/etc/agentix-agent/agent.toml` contains only non-secret settings. Systemd creates the service account and `/var/lib/agentix-agent`. Personal Sled and LanceDB data use separate persistent subdirectories. The required legacy ntfy block is unconfigured; this personal-only example does not provide notification delivery or Google integration. Merge existing legacy database/notification/Google settings if already using those features; do not replace a working legacy configuration blindly.

The example assumes the existing `ai.lan.disasm.us` nginx virtual host already has HTTPS/certificates configured. It adds only `/v1/assistant/`, preserves the full request path and bearer header, and proxies to the loopback agent listener. Keep port8765 private; the inference gateway remains a separate HTTP service at `ai.lan.disasm.us:11434`.

After applying the host configuration, inspect `systemctl status agentix-agent` and `journalctl -u agentix-agent`. An unauthenticated POST to `https://ai.lan.disasm.us/v1/assistant/tell` must return401, rather than502. Then configure the same HTTPS base URL and owner token in the Android app, explicitly enable the backend and save. Test grocery add/read/correction and a retry after a network interruption before changing the default assistant.

The example and generated TOML were evaluated with the project's pinned NixOS module. It has not been deployed or tested on the phone. The last public-route check returned502.
