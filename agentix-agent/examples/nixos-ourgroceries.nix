# Standalone alternative to nixos-personal-assistant.nix; do not import both.
{
  inputs,
  pkgs,
  lib,
  ...
}: let
  # Explicit choice: WireGuard-only ingress can leave this false.
  requireOwnerToken = false;
in {
  imports = [inputs.agentix.nixosModules.agentix];
  services.agentix.agent = {
    enable = true;
    package = inputs.agentix.packages.${pkgs.stdenv.hostPlatform.system}.agentix-agent;
    configFile = "/etc/agentix-agent/agent.toml";
    # Runtime secret file with OURGROCERIES_EMAIL and OURGROCERIES_PASSWORD.
    # Also supply AGENTIX_ASSISTANT_TOKEN when requireOwnerToken is true.
    environmentFile = "/run/secrets/agentix-agent.env";
  };
  environment.etc."agentix-agent/agent.toml".source = (pkgs.formats.toml {}).generate "agentix-agent.toml" {
    bind_addr = "127.0.0.1:8767";
    gateway_url = "http://ai.lan.disasm.us:11434";
    local_model = "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf";
    db_path = "/var/lib/agentix-agent/legacy";
    ntfy = {
      url = "";
      topic = "";
    };
    scheduler = {};
    assistant =
      {
        db_path = "/var/lib/agentix-agent/personal";
        index_path = "/var/lib/agentix-agent/personal-index";
        inference_timeout_secs = 30;
        ourgroceries.list_id = "REPLACE_WITH_EXISTING_LIST_ID";
      }
      // lib.optionalAttrs requireOwnerToken {token_env = "AGENTIX_ASSISTANT_TOKEN";};
  };
  services.nginx.enable = true;
  # Merge with the existing TLS vhost; ingress must remain restricted to WG peers.
  services.nginx.virtualHosts."ai.lan.disasm.us".locations."/v1/assistant/" = {
    proxyPass = "http://127.0.0.1:8767";
    extraConfig = ''
      proxy_set_header Authorization $http_authorization;
      proxy_read_timeout 150s;
      client_max_body_size 16k;
    '';
  };
}
