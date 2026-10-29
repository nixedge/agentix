# Import this module on the host running nginx and agentix-agent.
# Supply inputs through specialArgs; pin inputs.agentix to feature025 or newer.
{
  inputs,
  pkgs,
  config,
  lib,
  ...
}: {
  imports = [inputs.agentix.nixosModules.agentix];

  services.agentix.agent = {
    enable = true;
    package = inputs.agentix.packages.${pkgs.stdenv.hostPlatform.system}.agentix-agent;
    configFile = "/etc/agentix-agent/agent.toml";
    # Optional bearer auth: provide a runtime environmentFile containing
    # AGENTIX_ASSISTANT_TOKEN, or explicitly configure assistant.token_env below.
  };

  environment.etc."agentix-agent/agent.toml".source = (pkgs.formats.toml {}).generate "agentix-agent.toml" {
    gateway_url = "http://ai.lan.disasm.us:11434";
    local_model = "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf";
    bind_addr = "127.0.0.1:8767";
    # Separate legacy store; preserve your current path if already using legacy todos.
    db_path = "/var/lib/agentix-agent/legacy";
    assistant =
      {
        db_path = "/var/lib/agentix-agent/personal";
        index_path = "/var/lib/agentix-agent/personal-index";
        inference_timeout_secs = 30;
      }
      // lib.optionalAttrs (config.services.agentix.agent.environmentFile != null) {
        token_env = "AGENTIX_ASSISTANT_TOKEN";
      };
    # Required legacy configuration; notification delivery is unconfigured.
    # Personal task storage does not deliver reminders.
    ntfy = {
      url = "";
      topic = "";
    };
    scheduler = {};
  };

  services.nginx.enable = true;
  # Merge into the existing WireGuard-only HTTPS vhost; retain its TLS setup.
  # The existing listener/firewall must restrict ingress to authorized WG peers.
  services.nginx.virtualHosts."ai.lan.disasm.us".locations."/v1/assistant/" = {
    proxyPass = "http://127.0.0.1:8767";
    extraConfig = ''
      proxy_set_header Authorization $http_authorization;
      proxy_read_timeout 150s;
      client_max_body_size 16k;
    '';
  };
}
