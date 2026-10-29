# Import this module on the host running nginx and agentix-agent.
# Supply inputs through specialArgs; pin inputs.agentix to feature025 or newer.
{
  inputs,
  pkgs,
  ...
}: {
  imports = [inputs.agentix.nixosModules.agentix];

  services.agentix.agent = {
    enable = true;
    package = inputs.agentix.packages.${pkgs.stdenv.hostPlatform.system}.agentix-agent;
    configFile = "/etc/agentix-agent/agent.toml";
    environmentFile = "/run/secrets/agentix-agent.env";
  };

  environment.etc."agentix-agent/agent.toml".source = (pkgs.formats.toml {}).generate "agentix-agent.toml" {
    gateway_url = "http://ai.lan.disasm.us:11434";
    local_model = "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Qwen2.5-7B-Instruct-Q8_0.gguf";
    bind_addr = "127.0.0.1:8765";
    # Separate legacy store; preserve your current path if already using legacy todos.
    db_path = "/var/lib/agentix-agent/legacy";
    assistant = {
      db_path = "/var/lib/agentix-agent/personal";
      index_path = "/var/lib/agentix-agent/personal-index";
      token_env = "AGENTIX_ASSISTANT_TOKEN";
      inference_timeout_secs = 30;
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
  # Merge into the existing HTTPS vhost; retain its TLS/certificate configuration.
  services.nginx.virtualHosts."ai.lan.disasm.us".locations."/v1/assistant/" = {
    proxyPass = "http://127.0.0.1:8765";
    extraConfig = ''
      proxy_set_header Authorization $http_authorization;
      proxy_read_timeout 150s;
      client_max_body_size 16k;
    '';
  };
}
