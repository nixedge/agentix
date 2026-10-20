{ ... }: {
  flake.nixosModules.agentix = {
    config,
    lib,
    pkgs,
    ...
  }: let
    cfg = config.services.agentix;
    socketDir = cfg.socketDir;
    llamaSock = "${socketDir}/llama.sock";
    qwenSock = "${socketDir}/qwen.sock";
    whisperSock = "${socketDir}/whisper.sock";

    anyEnabled = cfg.daemon.enable || cfg.llama.enable || cfg.qwen.enable || cfg.whisper.enable || cfg.agent.enable;
  in {
    options.services.agentix = {
      socketDir = lib.mkOption {
        type = lib.types.str;
        default = "/run/agentix";
        description = "Directory holding the Unix domain sockets for each backend.";
      };

      # ── Daemon ──────────────────────────────────────────────────────────────

      daemon = {
        enable = lib.mkEnableOption "agentix-daemon OpenAI-compatible HTTP gateway";

        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-daemon package to use.";
        };

        host = lib.mkOption {
          type = lib.types.str;
          default = "[::]";
          description = "Address the HTTP gateway binds to.";
        };

        port = lib.mkOption {
          type = lib.types.port;
          default = 11434;
          description = "Port for the OpenAI-compatible HTTP gateway.";
        };

        ollamaBaseUrl = lib.mkOption {
          type = lib.types.str;
          default = "http://localhost:11434";
          description = "Ollama instance used as fallback for embedding proxying.";
        };

        piperVoicesDir = lib.mkOption {
          type = lib.types.path;
          default = "/var/lib/agentix/voices";
          description = "Directory containing Piper TTS voice model files (.onnx + .onnx.json).";
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = ''
            File containing secret environment variables (one per line):
              ANTHROPIC_API_KEY=sk-ant-...
              OPENAI_API_KEY=sk-...
              OPENROUTER_API_KEY=sk-or-...
            Compatible with sops-nix / agenix EnvironmentFile patterns.
          '';
        };

        extraEnv = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
          description = "Additional environment variables passed verbatim to agentix-daemon.";
        };
      };

      # ── LlamaCpp backend ────────────────────────────────────────────────────

      llama = {
        enable = lib.mkEnableOption "agentix-llama LlamaCpp inference backend";

        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-llama package to use.";
        };

        modelsDir = lib.mkOption {
          type = lib.types.path;
          default = "/var/lib/agentix/models";
          description = "Directory where pulled GGUF blobs and manifests are stored.";
        };

        preloadedModels = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [];
          example = [ "registry.ollama.ai/library/deepseek-r1/7b" ];
          description = ''
            Models to pull (if not already present) and load into VRAM at startup.
            The API can load and unload models freely after startup.
          '';
        };

        gpuLayers = lib.mkOption {
          type = lib.types.int;
          default = -1;
          description = ''
            Number of model layers to offload to GPU (-1 = all layers).
            Set to 0 for CPU-only inference.
          '';
        };

        maxCtx = lib.mkOption {
          type = lib.types.ints.positive;
          default = 32768;
          example = 65536;
          description = "Maximum context window (tokens) per loaded model.";
        };

        maxLoadedModels = lib.mkOption {
          type = lib.types.ints.positive;
          default = 2;
          description = "Maximum number of models kept resident in VRAM simultaneously.";
        };

        vramLimitBytes = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.positive;
          default = null;
          example = 23000000000;
          description = "Hard VRAM cap in bytes. Null means no explicit limit.";
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "File containing additional secret environment variables for agentix-llama.";
        };

        extraEnv = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
          description = "Additional environment variables passed verbatim to agentix-llama.";
        };
      };

      # ── Qwen Flash Next worker ──────────────────────────────────────────────

      qwen = {
        enable = lib.mkEnableOption "agentix-qwen-worker isolated inference process";

        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-qwen-worker package to use.";
        };

        model = lib.mkOption {
          type = lib.types.str;
          default = "qwen3.8-flash-next";
          description = "Exact model identifier routed to the Qwen worker.";
        };

        modelDir = lib.mkOption {
          type = lib.types.path;
          default = "/var/lib/agentix/qwen";
          description = "Directory containing config.json, tokenizer assets and GGUF shards.";
        };

        shards = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = ["iq3s-00001.gguf" "iq3s-00002.gguf"];
          description = "GGUF shard filenames inside modelDir, in split order.";
        };

        expectedShardSha256 = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [
            "4c1eb2ceb4915e1192f4f386021897bde56a97f40a0bb78bb86465e0f7d2aca3"
            "316b46f3a2dbd68c900f43136ab9449f9dcc3725dfd8c794847c204bc161e113"
          ];
          description = "Publisher SHA256 values for the IQ3_S GGUF shards, in split order.";
        };

        context = lib.mkOption {
          type = lib.types.ints.positive;
          default = 32768;
          description = "Maximum admitted Qwen context, capped by the loaded model configuration.";
        };

        compute = lib.mkOption {
          type = lib.types.enum ["cuda" "cpu"];
          default = "cuda";
          description = "CUDA matrix execution throughout all blocks, or explicit slow CPU reference execution.";
        };

        cudaDevice = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 0;
          description = "GGML CUDA device index.";
        };

        cudaCacheBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 17179869184;
          description = "Encoded GPU weight-cache cap (16 GiB by default); startup also requires a 2 GiB device reserve.";
        };

        extraEnv = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
          description = "Additional environment variables passed to agentix-qwen-worker.";
        };
      };

      # ── agentix-agent home assistant ────────────────────────────────────────

      agent = {
        enable = lib.mkEnableOption "agentix-agent home assistant daemon";

        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-agent package to use.";
        };

        configFile = lib.mkOption {
          type = lib.types.path;
          description = ''
            Path to the agent.toml configuration file.  Secrets (Google OAuth
            tokens, ntfy topic) should be stored outside the Nix store.
          '';
        };

        user = lib.mkOption {
          type = lib.types.str;
          default = "agentix-agent";
          description = "User account under which agentix-agent runs.";
        };

        group = lib.mkOption {
          type = lib.types.str;
          default = "agentix-agent";
          description = "Group under which agentix-agent runs.";
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = ''
            Optional file containing secret environment variables passed to
            agentix-agent at runtime.  Compatible with sops-nix / agenix.
          '';
        };

        extraEnv = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
          description = "Additional environment variables passed verbatim to agentix-agent.";
        };
      };

      # ── Whisper STT backend ─────────────────────────────────────────────────

      whisper = {
        enable = lib.mkEnableOption "agentix-whisper speech-to-text backend";

        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-whisper package to use.";
        };

        modelsDir = lib.mkOption {
          type = lib.types.path;
          default = "/var/lib/agentix/models";
          description = "Directory where pulled whisper .bin models are stored.";
        };

        preloadedModels = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [];
          example = [ "ggml-base.en.bin" ];
          description = ''
            Models to pull (if not already present) and load at startup.
            The API can load and unload models freely after startup.
          '';
        };

        environmentFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "File containing additional secret environment variables for agentix-whisper.";
        };

        extraEnv = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
          description = "Additional environment variables passed verbatim to agentix-whisper.";
        };
      };
    };

    config = lib.mkMerge [
      # ── Shared: user, group, socket dir, model store ─────────────────────────
      (lib.mkIf anyEnabled {
        users.users.agentix = {
          isSystemUser = true;
          group = "agentix";
          home = "/var/lib/agentix";
          description = "agentix service user";
          # GPU access: render covers NVIDIA (nvidia-uvm) and AMD/Intel.
          # video is needed for some driver paths that check group membership.
          extraGroups = ["video" "render"];
        };

        users.groups.agentix = {};

        systemd.tmpfiles.rules = [
          # Socket directory — recreated on boot (under /run which is tmpfs)
          "d ${socketDir} 0750 agentix agentix -"
          # Persistent model store shared by llama and whisper backends
          "d /var/lib/agentix/models 0750 agentix agentix -"
        ];
      })

      # ── agentix-daemon ───────────────────────────────────────────────────────
      (lib.mkIf cfg.daemon.enable {
        systemd.services.agentix-daemon = {
          description = "agentix OpenAI-compatible inference gateway";
          wantedBy = ["multi-user.target"];
          after =
            ["network.target"]
            ++ lib.optional cfg.llama.enable "agentix-llama.service"
            ++ lib.optional cfg.qwen.enable "agentix-qwen.service"
            ++ lib.optional cfg.whisper.enable "agentix-whisper.service";
          # Soft ordering only — daemon handles 503 when backends are down
          wants =
            lib.optional cfg.llama.enable "agentix-llama.service"
            ++ lib.optional cfg.qwen.enable "agentix-qwen.service"
            ++ lib.optional cfg.whisper.enable "agentix-whisper.service";

          environment =
            {
              AGENTIX_GATEWAY_HOST = cfg.daemon.host;
              AGENTIX_GATEWAY_PORT = toString cfg.daemon.port;
              AGENTIX_LLAMA_SOCKET = llamaSock;
              AGENTIX_QWEN_SOCKET = qwenSock;
              AGENTIX_QWEN_MODEL = cfg.qwen.model;
              AGENTIX_WHISPER_SOCKET = whisperSock;
              OLLAMA_BASE_URL = cfg.daemon.ollamaBaseUrl;
              AGENTIX_PIPER_VOICES_DIR = toString cfg.daemon.piperVoicesDir;
            }
            // cfg.daemon.extraEnv;

          serviceConfig = {
            ExecStart = "${cfg.daemon.package}/bin/agentix-daemon";
            Restart = "on-failure";
            RestartSec = "5s";
            User = "agentix";
            Group = "agentix";
            StateDirectory = "agentix";
            StateDirectoryMode = "0750";
            EnvironmentFile = lib.mkIf (cfg.daemon.environmentFile != null) cfg.daemon.environmentFile;
          };
        };

        systemd.tmpfiles.rules = [
          "d ${toString cfg.daemon.piperVoicesDir} 0750 agentix agentix -"
        ];
      })

      # ── agentix-qwen-worker ─────────────────────────────────────────────────
      (lib.mkIf cfg.qwen.enable {
        systemd.services.agentix-qwen = {
          description = "agentix Qwen Flash Next inference worker";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          environment = {
            AGENTIX_QWEN_SOCKET = qwenSock;
            AGENTIX_QWEN_MODEL = cfg.qwen.model;
            AGENTIX_QWEN_MODEL_DIR = toString cfg.qwen.modelDir;
            AGENTIX_QWEN_GGUF_SHARDS = lib.concatStringsSep ":" (map (name: "${toString cfg.qwen.modelDir}/${name}") cfg.qwen.shards);
            AGENTIX_QWEN_EXPECTED_SHARD_SHA256 = lib.concatStringsSep ":" cfg.qwen.expectedShardSha256;
            AGENTIX_QWEN_CONTEXT = toString cfg.qwen.context;
            AGENTIX_QWEN_COMPUTE = cfg.qwen.compute;
            AGENTIX_QWEN_CUDA_DEVICE = toString cfg.qwen.cudaDevice;
            AGENTIX_QWEN_CUDA_CACHE_BYTES = toString cfg.qwen.cudaCacheBytes;
          } // cfg.qwen.extraEnv;

          serviceConfig = {
            ExecStart = "${cfg.qwen.package}/bin/agentix-qwen-worker";
            Restart = "on-failure";
            RestartSec = "10s";
            User = "agentix";
            Group = "agentix";
            StateDirectory = "agentix";
            StateDirectoryMode = "0750";
            PrivateDevices = false;
          };
        };

        systemd.tmpfiles.rules = [
          "d ${toString cfg.qwen.modelDir} 0750 agentix agentix -"
        ];
      })

      # ── agentix-llama ────────────────────────────────────────────────────────
      (lib.mkIf cfg.llama.enable {
        warnings = lib.optional (pkgs.config.cudaCapabilities or [] == []) ''
          services.agentix.llama is enabled but nixpkgs.config.cudaCapabilities is not set.
          The package was built without GPU-specific CUDA kernels; inference will run on CPU.
          Set this in your NixOS configuration, for example:
            nixpkgs.config.cudaCapabilities = [ "8.6" ];  # RTX 3090
            nixpkgs.config.cudaCapabilities = [ "8.9" ];  # RTX 4090
        '';

        systemd.services.agentix-llama = {
          description = "agentix LlamaCpp inference backend";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          environment =
            {
              AGENTIX_LLAMA_SOCKET = llamaSock;
              AGENTIX_MODELS_DIR = toString cfg.llama.modelsDir;
              AGENTIX_MAX_CTX = toString cfg.llama.maxCtx;
              AGENTIX_MAX_LOADED_MODELS = toString cfg.llama.maxLoadedModels;
            }
            // lib.optionalAttrs (cfg.llama.gpuLayers >= 0) {
              AGENTIX_GPU_LAYERS = toString cfg.llama.gpuLayers;
            }
            // lib.optionalAttrs (cfg.llama.vramLimitBytes != null) {
              AGENTIX_VRAM_LIMIT_BYTES = toString cfg.llama.vramLimitBytes;
            }
            // lib.optionalAttrs (cfg.llama.preloadedModels != []) {
              AGENTIX_LLAMA_MODELS = lib.concatStringsSep "," cfg.llama.preloadedModels;
            }
            // cfg.llama.extraEnv;

          serviceConfig = {
            ExecStart = "${cfg.llama.package}/bin/agentix-llama";
            # Explicit /control/shutdown exits cleanly (code 0) — no auto-restart.
            # Crashes (non-zero exit) do restart.
            Restart = "on-failure";
            RestartSec = "5s";
            User = "agentix";
            Group = "agentix";
            StateDirectory = "agentix";
            StateDirectoryMode = "0750";
            # GPU passthrough: any DeviceAllow entry installs a cgroup BPF filter
            # that blocks everything not listed, breaking CUDA even for world-readable
            # device nodes. Leave PrivateDevices off and add no DeviceAllow.
            PrivateDevices = false;
            EnvironmentFile = lib.mkIf (cfg.llama.environmentFile != null) cfg.llama.environmentFile;
          };
        };

        systemd.tmpfiles.rules = lib.optionals (toString cfg.llama.modelsDir != "/var/lib/agentix/models") [
          "d ${toString cfg.llama.modelsDir} 0750 agentix agentix -"
        ];
      })

      # ── agentix-whisper ──────────────────────────────────────────────────────
      (lib.mkIf cfg.whisper.enable {
        systemd.services.agentix-whisper = {
          description = "agentix Whisper speech-to-text backend";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          environment =
            {
              AGENTIX_WHISPER_SOCKET = whisperSock;
              AGENTIX_MODELS_DIR = toString cfg.whisper.modelsDir;
            }
            // lib.optionalAttrs (cfg.whisper.preloadedModels != []) {
              AGENTIX_WHISPER_MODELS = lib.concatStringsSep "," cfg.whisper.preloadedModels;
            }
            // cfg.whisper.extraEnv;

          serviceConfig = {
            ExecStart = "${cfg.whisper.package}/bin/agentix-whisper";
            Restart = "on-failure";
            RestartSec = "5s";
            User = "agentix";
            Group = "agentix";
            StateDirectory = "agentix";
            StateDirectoryMode = "0750";
            PrivateDevices = false;
            EnvironmentFile = lib.mkIf (cfg.whisper.environmentFile != null) cfg.whisper.environmentFile;
          };
        };

        systemd.tmpfiles.rules = lib.optionals (toString cfg.whisper.modelsDir != "/var/lib/agentix/models") [
          "d ${toString cfg.whisper.modelsDir} 0750 agentix agentix -"
        ];
      })

      # ── agentix-agent ────────────────────────────────────────────────────────
      (lib.mkIf cfg.agent.enable {
        users.users.${cfg.agent.user} = lib.mkDefault {
          isSystemUser = true;
          group = cfg.agent.group;
          home = "/var/lib/agentix-agent";
          description = "agentix-agent home assistant service user";
        };

        users.groups.${cfg.agent.group} = lib.mkDefault {};

        systemd.tmpfiles.rules = [
          "d /var/lib/agentix-agent 0750 ${cfg.agent.user} ${cfg.agent.group} -"
        ];

        systemd.services.agentix-agent = {
          description = "agentix-agent home assistant daemon";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          environment = cfg.agent.extraEnv;

          serviceConfig = {
            ExecStart = "${cfg.agent.package}/bin/agentix-agent --config ${cfg.agent.configFile} daemon";
            Restart = "on-failure";
            RestartSec = "5s";
            User = cfg.agent.user;
            Group = cfg.agent.group;
            WorkingDirectory = "/var/lib/agentix-agent";
            StateDirectory = "agentix-agent";
            StateDirectoryMode = "0750";
            EnvironmentFile = lib.mkIf (cfg.agent.environmentFile != null) cfg.agent.environmentFile;
          };
        };
      })
    ];
  };
}
