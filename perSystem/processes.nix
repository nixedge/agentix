{inputs, ...}: {
  perSystem = {
    pkgs,
    config,
    ...
  }: let
    pgService = {
      enable = true;
      # pg_search (BM25) + pgvector are in postgresql17Packages, not postgresql_17.pkgs,
      # so we call withPackages directly rather than using the extensions option.
      package = pkgs.postgresql_17.withPackages (ps: [
        pkgs.postgresql17Packages.pg_search
        pkgs.postgresql17Packages.pgvector
      ]);

      # pg_search must be preloaded
      settings.shared_preload_libraries = "pg_search";

      initialDatabases = [
        {
          name = "codebase";
          schemas = [../scripts/schema.sql];
        }
      ];
    };
  in {
    # ── PostgreSQL (ParadeDB) + Ollama ────────────────────────────────────────
    process-compose.dev = {
      imports = [inputs.services-flake.processComposeModules.default];

      services.postgres."pg" = pgService;

      # jina-code-embeddings-1.5b: code-specific, Qwen2.5-Coder base, 32k ctx.
      # Ollama returns 1536-dim vectors.
      services.ollama."llm" = {
        enable = true;
        acceleration = "cuda";
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

    # ── PostgreSQL (ParadeDB) + llama-server ──────────────────────────────────
    process-compose.dev-llama = {
      imports = [inputs.services-flake.processComposeModules.default];

      services.postgres."pg" = pgService;

      settings.processes.llama-server = {
        # Same model as the Ollama variant, loaded directly from HuggingFace.
        # Verify the exact filename at: https://huggingface.co/jinaai/jina-code-embeddings-1.5b-GGUF
        # --pooling last: this model's GGUF doesn't declare a pooling type, so llama.cpp
        # defaults to "none" (per-token), which /v1/embeddings rejects as not OAI-compatible.
        # jina-code-embeddings-1.5b's model card specifies last-token pooling.
        command = ''
          ${pkgs.llama-cpp}/bin/llama-server \
            --hf-repo jinaai/jina-code-embeddings-1.5b-GGUF \
            --hf-file jina-code-embeddings-1.5b-Q8_0.gguf \
            --embedding --pooling last --ctx-size 0 --port 8080 -ngl 99
        '';
        readiness_probe = {
          http_get = {
            host = "127.0.0.1";
            port = 8080;
            path = "/health";
          };
          initial_delay_seconds = 5;
          period_seconds = 3;
        };
      };
    };

    # ── PostgreSQL (ParadeDB) only ────────────────────────────────────────────
    # For pointing at an embedding server you run yourself (Ollama or llama.cpp).
    process-compose.postgres = {
      imports = [inputs.services-flake.processComposeModules.default];

      services.postgres."pg" = pgService;
    };
  };
}
