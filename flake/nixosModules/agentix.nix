{...}: {
  flake.nixosModules.agentix = {
    config,
    lib,
    pkgs,
    ...
  }: let
    cfg = config.services.agentix;
    socketDir = cfg.socketDir;
    llamaSock = "${socketDir}/llama.sock";
    whisperSock = "${socketDir}/whisper.sock";

    anyEnabled = cfg.daemon.enable || cfg.llama.enable || cfg.whisper.enable || cfg.agent.enable;
  in {
    options.services.agentix = {
      storage = {
        enable = lib.mkEnableOption "Agentix shared LanceDB storage API";
        package = lib.mkOption {
          type = lib.types.package;
          description = "The agentix-store-server package to use.";
        };
        dataDir = lib.mkOption {
          type = lib.types.str;
          default = "/var/lib/agentix-store/index";
          description = "Absolute persistent directory owned by the storage service.";
        };
        host = lib.mkOption {
          type = lib.types.str;
          default = "127.0.0.1";
          description = "Listen address. Use a private address or a TLS reverse proxy for remote access.";
        };
        port = lib.mkOption {
          type = lib.types.port;
          default = 8091;
          description = "Storage HTTP API port.";
        };
        tokenFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "Optional runtime bearer-token file. Null allows LAN access without authentication. When set, use a /run secrets path, never a Nix store secret.";
        };
        embedModel = lib.mkOption {
          type = lib.types.str;
          default = "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0";
          description = "Embedding identity checked against the index and every client.";
        };
        embedDimensions = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1536;
          description = "Stored embedding dimensions; must match all clients.";
        };
        openFirewall = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = "Open the API TCP port. Prefer interface-scoped firewall rules for a private network.";
        };
      };
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
          example = ["registry.ollama.ai/library/deepseek-r1/7b"];
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
          example = ["ggml-base.en.bin"];
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
      (lib.mkIf cfg.storage.enable {
        assertions = [
          {
            assertion = lib.hasPrefix "/" cfg.storage.dataDir;
            message = "services.agentix.storage.dataDir must be absolute.";
          }
          {
            assertion = cfg.storage.tokenFile == null || !(lib.hasPrefix "/nix/store/" (toString cfg.storage.tokenFile));
            message = "services.agentix.storage.tokenFile must refer to a runtime secret outside the Nix store.";
          }
        ];
        users.groups.agentix-store = {};
        users.users.agentix-store = {
          isSystemUser = true;
          group = "agentix-store";
          home = "/var/lib/agentix-store";
        };
        systemd.tmpfiles.rules = ["d ${cfg.storage.dataDir} 0750 agentix-store agentix-store -"];
        networking.firewall.allowedTCPPorts = lib.optionals cfg.storage.openFirewall [cfg.storage.port];
        systemd.services.agentix-store = {
          description = "Agentix shared LanceDB storage API";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];
          environment =
            {
              AGENTIX_LANCE_PATH = cfg.storage.dataDir;
              AGENTIX_STORE_HOST = cfg.storage.host;
              AGENTIX_STORE_PORT = toString cfg.storage.port;
              EMBED_MODEL = cfg.storage.embedModel;
              EMBED_DIMENSIONS = toString cfg.storage.embedDimensions;
            }
            // lib.optionalAttrs (cfg.storage.tokenFile != null) {
              AGENTIX_STORE_TOKEN_FILE = "%d/store-token";
            };
          serviceConfig = {
            ExecStart = "${cfg.storage.package}/bin/agentix-store-server";
            User = "agentix-store";
            Group = "agentix-store";
            StateDirectory = "agentix-store";
            StateDirectoryMode = "0750";
            LoadCredential = lib.mkIf (cfg.storage.tokenFile != null) "store-token:${cfg.storage.tokenFile}";
            Restart = "on-failure";
            RestartSec = "5s";
            TimeoutStopSec = "300s";
            UMask = "0027";
            NoNewPrivileges = true;
            PrivateTmp = true;
            ProtectHome = true;
            ProtectSystem = "strict";
            ReadWritePaths = [cfg.storage.dataDir];
          };
        };
      })
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
            ++ lib.optional cfg.whisper.enable "agentix-whisper.service";
          # Soft ordering only — daemon handles 503 when backends are down
          wants =
            lib.optional cfg.llama.enable "agentix-llama.service"
            ++ lib.optional cfg.whisper.enable "agentix-whisper.service";

          environment =
            {
              AGENTIX_GATEWAY_HOST = cfg.daemon.host;
              AGENTIX_GATEWAY_PORT = toString cfg.daemon.port;
              AGENTIX_LLAMA_SOCKET = llamaSock;
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
