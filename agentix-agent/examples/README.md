# Personal assistant on NixOS

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
