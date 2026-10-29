{inputs, ...}: {
  perSystem = {
    pkgs,
    config,
    ...
  }: {
    process-compose.dev = {
      imports = [inputs.services-flake.processComposeModules.default];

      # ── Ollama (embeddings + optional LLM) ───────────────────────────────
      services.ollama."llm" = {
        enable = true;
        acceleration = "cuda";
        # jina-code-embeddings-1.5b: code-specific, Qwen2.5-Coder base, 32k ctx.
        # Ollama returns 1536-dim vectors.
        models = ["hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0"];
      };

      # ── agentix-daemon (OpenAI-compatible gateway) ────────────────────────
      settings.processes."agentix-daemon" = {
        command = "${config.packages.agentix-daemon}/bin/agentix-daemon";
        environment = {
          RUST_LOG = "agentix_daemon=debug,info";
          AGENTIX_GATEWAY_PORT = "11430";
          AGENTIX_DEFAULT_ROUTE = "anthropic";
          AGENTIX_MODELS_DIR = "/var/lib/agentix/models";
          AGENTIX_MAX_LOADED_MODELS = "2";
          OLLAMA_BASE_URL = "http://localhost:11434";
        };
        readiness_probe.http_get = {
          host = "localhost";
          port = 11430;
          path = "/health";
        };
      };
    };
  };
}
