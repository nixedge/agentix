{inputs, ...}: {
  perSystem = {
    inputs',
    lib,
    pkgs,
    ...
  }: let
    toolchain = inputs'.fenix.packages.stable.toolchain;
    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain toolchain;
    workspace = import ../nix/isolated-workspace.nix {
      inherit lib pkgs;
      root = ./..;
      normalizeLock = import ../nix/normalize-cargo-lock.nix {inherit pkgs craneLib toolchain;};
    };
    scopes = lib.mapAttrs (name: _: workspace.scope [name]) workspace.members;
    argsFor = roots: let
      scope = workspace.scope roots;
      has = name: builtins.elem name scope.packageNames;
      onnx = has "ort-sys";
      bindgen = has "bindgen";
      cmake = has "cmake";
      openssl = has "openssl-sys" || onnx;
      alsa = has "alsa-sys";
    in
      {
        pname = builtins.head roots;
        version = "0.1.0";
        src = scope.src;
        cargoLockParsed = scope.scopedLock;
        cargoVendorDir = craneLib.vendorCargoDeps {cargoLockParsed = scope.scopedLock;};
        strictDeps = true;
        doCheck = false;
        nativeBuildInputs =
          lib.optional (has "pkg-config") pkgs.pkg-config
          ++ lib.optionals bindgen [pkgs.clang]
          ++ lib.optionals cmake [pkgs.cmake pkgs.ninja]
          ++ lib.optional (has "prost-build" || has "protobuf-src") pkgs.protobuf
          ++ lib.optional onnx pkgs.autoPatchelfHook;
        buildInputs =
          lib.optional openssl pkgs.openssl
          ++ lib.optional onnx pkgs.onnxruntime
          ++ lib.optional bindgen pkgs.libclang.lib
          ++ lib.optional alsa pkgs.alsa-lib
          ++ lib.optional (bindgen || cmake || onnx) pkgs.stdenv.cc.cc.lib;
      }
      // lib.optionalAttrs bindgen {LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";}
      // lib.optionalAttrs cmake {
        CMAKE_GENERATOR = "Ninja";
        CMAKE_MAKE_PROGRAM = "${pkgs.ninja}/bin/ninja";
      }
      // lib.optionalAttrs onnx {
        ORT_DYLIB_PATH = "${pkgs.onnxruntime}/lib/libonnxruntime.so";
        ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";
        ORT_PREFER_DYNAMIC_LINK = "1";
        LD_LIBRARY_PATH = "${pkgs.openssl.out}/lib:${pkgs.onnxruntime}/lib:${pkgs.stdenv.cc.cc.lib}/lib";
      };

    cudaPackages = pkgs.cudaPackages_12;
    libcublasStatic = pkgs.lib.getOutput "static" cudaPackages.libcublas;
    cudaCapabilities = pkgs.config.cudaCapabilities or [];
    withCuda = cudaCapabilities != [];
    cudaArgs = base:
      lib.optionalAttrs withCuda {
        nativeBuildInputs = base.nativeBuildInputs ++ [cudaPackages.cuda_nvcc cudaPackages.cuda_cudart];
        buildInputs = base.buildInputs ++ [cudaPackages.cuda_cudart cudaPackages.libcublas libcublasStatic];
        CMAKE_CUDA_COMPILER = "${cudaPackages.cuda_nvcc}/bin/nvcc";
        CMAKE_CUDA_ARCHITECTURES = builtins.concatStringsSep ";" (map (c: builtins.replaceStrings ["."] [""] c) cudaCapabilities);
        CUDA_HOME = "${cudaPackages.cudatoolkit}";
        CUDA_PATH = "${cudaPackages.cudatoolkit}";
        CUDA_TOOLKIT_ROOT_DIR = "${cudaPackages.cudatoolkit}";
        RUSTFLAGS = "-L ${cudaPackages.cudatoolkit}/lib -L ${cudaPackages.cudatoolkit}/lib/stubs -L ${cudaPackages.cuda_cudart}/lib -L ${libcublasStatic}/lib";
      };
    packageArgs = name: let
      base = argsFor [name];
    in
      base // lib.optionalAttrs (name == "agentix-llama") (cudaArgs base);
    extraArgs = name: "--locked --package ${name}" + lib.optionalString (name == "agentix-llama" && withCuda) " --features cuda";
    depsFor = name:
      craneLib.buildDepsOnly ((packageArgs name)
        // {
          src = scopes.${name}.dependencySrc;
          inherit (scopes.${name}) extraDummyScript;
          cargoExtraArgs = extraArgs name;
        });
    dependencyArtifacts = lib.genAttrs (builtins.attrNames workspace.members) depsFor;
    workspaceLockCheck = import ../nix/workspace-lock-check.nix {inherit pkgs lib toolchain craneLib scopes;};
    buildMember = name: overrides:
      craneLib.buildPackage ((packageArgs name)
        // {
          cargoArtifacts = dependencyArtifacts.${name};
          cargoExtraArgs = extraArgs name;
          autoPatchelfIgnoreMissingDeps = lib.optional (name == "agentix-llama" && withCuda) "libcuda.so.1";
          passthru.isolation = {
            members = scopes.${name}.selectedMembers;
            dependencyArtifacts = dependencyArtifacts.${name};
            source = scopes.${name}.src;
          };
        }
        // overrides);
    memberPackages = lib.mapAttrs (name: _: buildMember name {}) workspace.members;
    llamaCpuArgs = argsFor ["agentix-llama"];
    llamaCpuDeps = craneLib.buildDepsOnly (llamaCpuArgs
      // {
        src = scopes.agentix-llama.dependencySrc;
        inherit (scopes.agentix-llama) extraDummyScript;
        cargoExtraArgs = "--locked --package agentix-llama";
      });
    llamaCpuPackage = craneLib.buildPackage (llamaCpuArgs
      // {
        cargoArtifacts = llamaCpuDeps;
        cargoExtraArgs = "--locked --package agentix-llama";
      });
    checkFor = name: selection:
      craneLib.cargoTest ((packageArgs name)
        // {
          pname = "${name}-tests";
          doCheck = true;
          cargoArtifacts = dependencyArtifacts.${name};
          cargoExtraArgs = "${extraArgs name} ${selection}";
        });
    agentixDaemonPkg = memberPackages.agentix-daemon;
    agentixWhisperPkg = memberPackages.agentix-whisper;
    agentixLlamaPkg = memberPackages.agentix-llama;
    axPkg = memberPackages.agentix-ax;
    agentixMcpServerPkg = memberPackages.agentix-mcp-server.overrideAttrs (_: {meta.mainProgram = "mcp-server";});
    agentixIndexerPkg = memberPackages.agentix-indexer.overrideAttrs (_: {meta.mainProgram = "ingest";});
    assistantCheck = craneLib.cargoTest ((argsFor ["agentix-assistant"])
      // {
        pname = "agentix-assistant-tests";
        doCheck = true;
        cargoArtifacts = dependencyArtifacts.agentix-assistant;
        cargoExtraArgs = "--locked --package agentix-assistant-core --package agentix-assistant-sled --package agentix-assistant";
      });
    assistantLanceCheck = checkFor "agentix-assistant-lance" "";
    agentixStoreCheck = checkFor "agentix-store" "";
    agentixStoreServerPkg = buildMember "agentix-store" {
      pname = "agentix-store-server";
      cargoExtraArgs = "${extraArgs "agentix-store"} --bin agentix-store-server";
      meta.mainProgram = "agentix-store-server";
    };
    agentixAgentPkg = memberPackages.agentix-agent;
    assistantApiCheck = checkFor "agentix-agent" "assistant::";
    agentixVoicePkg = memberPackages.agentix-voice;
    jailPackage = name:
      buildMember "agentix-jails" {
        pname = name;
        cargoExtraArgs = "${extraArgs "agentix-jails"} --bin ${name}";
      };
    claudeJailUnwrapped = jailPackage "claude-jail";
    ghJailClientPkg = (jailPackage "gh-jail-client").overrideAttrs (_: {
      postInstall = ''mv $out/bin/gh-jail-client $out/bin/gh'';
    });
    ghJailServerPkg = jailPackage "gh-jail-server";
    axJailUnwrapped = jailPackage "ax-jail";
    codexJailUnwrapped = jailPackage "codex-jail";
    axJailSanityCheck = pkgs.writeShellScriptBin "ax-jail-check" ''
      #!/usr/bin/env bash
      set -euo pipefail

      PASS=0
      FAIL=0

      check() {
        local label="$1"
        local result="$2"
        local ok="$3"
        if [ "$ok" = "1" ]; then
          echo "[OK]   $label: $result"
          PASS=$((PASS + 1))
        else
          echo "[FAIL] $label: $result"
          FAIL=$((FAIL + 1))
        fi
      }

      WT=$(git rev-parse --show-toplevel 2>&1) && check "git rev-parse --show-toplevel" "$WT" 1 || check "git rev-parse --show-toplevel" "$WT" 0
      STATUS=$(git status 2>&1) && check "git status" "ok" 1 || check "git status" "$STATUS" 0
      LOG=$(git log -1 --oneline 2>&1) && check "git log -1" "$LOG" 1 || check "git log -1" "$LOG" 0

      COMMON=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || echo "")
      if [ -z "$COMMON" ]; then
        check "hooks mask" "cannot determine git-common-dir" 0
      else
        HOOKS_CONTENT=$(ls "$COMMON/hooks" 2>&1)
        if [ -z "$HOOKS_CONTENT" ]; then
          check "hooks mask" "empty" 1
        else
          check "hooks mask" "NOT empty: $HOOKS_CONTENT" 0
        fi
      fi

      if [ -z "$COMMON" ]; then
        check "config mask" "cannot determine git-common-dir" 0
      else
        CONFIG_OUT=$(git config --file "$COMMON/config" core.fsmonitor testvalue 2>&1 || true)
        if echo "$CONFIG_OUT" | grep -qi 'read.only\|permission\|cannot open'; then
          check "config mask" "write correctly denied" 1
        else
          check "config mask" "write was NOT denied (config not masked!)" 0
        fi
      fi

      KEYS=$(env | grep -E 'ANTHROPIC_API_KEY|OPENAI_API_KEY|OPENROUTER_API_KEY' || true)
      if [ -z "$KEYS" ]; then
        check "no API keys in environment" "clean" 1
      else
        check "no API keys in environment" "FOUND: $KEYS" 0
      fi

      echo ""
      if [ "$FAIL" -eq 0 ]; then
        echo "All $PASS check(s) passed."
        exit 0
      else
        echo "$FAIL check(s) FAILED."
        exit 1
      fi
    '';

    axJailBinDir = pkgs.buildEnv {
      name = "ax-jail-tools";
      pathsToLink = ["/bin"];
      paths = [
        pkgs.nix
        pkgs.git
        pkgs.gh
        pkgs.curl
        pkgs.bash
        pkgs.python3
        pkgs.direnv
        pkgs.coreutils
        pkgs.findutils
        pkgs.jq
        pkgs.gnused
        agentixIndexerPkg
        agentixMcpServerPkg
        axPkg
        axJailSanityCheck
      ];
    };

    codexJailBinDir = pkgs.buildEnv {
      name = "codex-jail-tools";
      pathsToLink = ["/bin"];
      paths = [
        inputs'.llm-agents.packages.codex
        pkgs.nix
        pkgs.git
        ghJailClientPkg
        pkgs.curl
        pkgs.bash
        pkgs.python3
        pkgs.direnv
        pkgs.coreutils
        pkgs.findutils
        pkgs.jq
        pkgs.gnugrep
        pkgs.ripgrep
        pkgs.gnused
        pkgs.openssh
        agentixIndexerPkg
        agentixMcpServerPkg
      ];
    };

    # Merged ~/bin for the jail. buildEnv with pathsToLink=["/bin"] handles
    # multi-binary packages (coreutils, findutils) correctly; all symlinks
    # point into /nix/store which is bind-mounted read-only inside the jail.
    claudeJailBinDir = pkgs.buildEnv {
      name = "claude-jail-tools";
      pathsToLink = ["/bin"];
      paths = [
        inputs'.llm-agents.packages.claude-code
        pkgs.nix
        pkgs.git
        ghJailClientPkg
        pkgs.curl
        pkgs.bash
        pkgs.python3
        pkgs.direnv
        pkgs.coreutils
        pkgs.findutils
        pkgs.jq
        pkgs.gnugrep
        pkgs.ripgrep
        pkgs.gnused
        pkgs.openssh
        agentixIndexerPkg
        agentixMcpServerPkg
      ];
    };
  in {
    checks.package-binary-selection = let
      selections = [
        [agentixStoreServerPkg "agentix-store" "agentix-store-server"]
        [claudeJailUnwrapped "agentix-jails" "claude-jail"]
        [ghJailClientPkg "agentix-jails" "gh-jail-client"]
        [ghJailServerPkg "agentix-jails" "gh-jail-server"]
        [axJailUnwrapped "agentix-jails" "ax-jail"]
        [codexJailUnwrapped "agentix-jails" "codex-jail"]
      ];
      correctSelection = selection:
        lib.hasInfix
        "--locked --package ${builtins.elemAt selection 1} --bin ${builtins.elemAt selection 2}"
        (builtins.head selection).buildPhase;
    in
      assert lib.assertMsg (lib.all correctSelection selections) "Crane binary variants must select their package and binary in the generated build command";
        pkgs.runCommand "package-binary-selection" {} ''
          touch "$out"
        '';
    checks.workspace-locks = workspaceLockCheck;
    checks.assistant-core = assistantCheck;
    checks.assistant-lance = assistantLanceCheck;
    checks.assistant-api = assistantApiCheck;
    checks.agentix-store = agentixStoreCheck;

    devShells.assistant = pkgs.mkShell {
      packages = [toolchain pkgs.alejandra];
      nativeBuildInputs = (argsFor ["agentix-agent"]).nativeBuildInputs;
      buildInputs = (argsFor ["agentix-agent"]).buildInputs;
      LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
      LD_LIBRARY_PATH = (argsFor ["agentix-agent"]).LD_LIBRARY_PATH or "";
    };
    packages =
      memberPackages
      // {
        workspace-locks = workspaceLockCheck;
        agentix-llama-cpu = llamaCpuPackage;
        assistant-core = assistantCheck;
        assistant-lance = assistantLanceCheck;
        agentix-agent = agentixAgentPkg;
        assistant-api = assistantApiCheck;

        agentix-voice = agentixVoicePkg;

        agentix-whisper = agentixWhisperPkg;

        agentix-llama = pkgs.writeShellScriptBin "agentix-llama" ''
          export LD_LIBRARY_PATH="/run/opengl-driver/lib:${cudaPackages.cuda_cudart}/lib:${cudaPackages.libcublas}/lib:''${LD_LIBRARY_PATH:-}"
          exec ${agentixLlamaPkg}/bin/agentix-llama "$@"
        '';

        agentix-mcp-server = agentixMcpServerPkg;

        agentix-indexer = agentixIndexerPkg;
        agentix-store = agentixStoreCheck;
        agentix-store-server = agentixStoreServerPkg;

        # Wrapper that sets LD_LIBRARY_PATH so libcuda.so.1 (NVIDIA driver API,
        # not in the Nix store) is found at runtime. Also fixes the nix run binary
        # name: crane names derivations after the root crate (mcp-server-0.1.0),
        # so nix run .#agentix-daemon would try to exec 'mcp-server' without this.
        agentix-daemon = pkgs.writeShellScriptBin "agentix-daemon" ''
          export AGENTIX_PIPER_BIN="${pkgs.piper-tts}/bin/piper"
          export AGENTIX_PIPER_VOICES_DIR="''${AGENTIX_PIPER_VOICES_DIR:-/var/lib/agentix/voices}"
          exec ${agentixDaemonPkg}/bin/agentix-daemon "$@"
        '';

        ax = axPkg;

        # Wrapper script sets env vars the Rust binary reads, then execs it.
        claude-jail = pkgs.writeShellScriptBin "claude-jail" ''
          # buildEnv puts merged symlinks under bin/
          export CLAUDE_JAIL_BIN_DIR="${claudeJailBinDir}/bin"
          export CLAUDE_JAIL_BWRAP="${pkgs.bubblewrap}/bin/bwrap"
          export CLAUDE_JAIL_GH_SERVER="${ghJailServerPkg}/bin/gh-jail-server"
          # Nix store cacert bundle — works even if /etc/ssl is absent on the host
          export NIX_SSL_CERT_FILE="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
          # Enable nix-command and flakes inside the jail
          export NIX_CONFIG="extra-experimental-features = nix-command flakes"
          exec ${claudeJailUnwrapped}/bin/claude-jail "$@"
        '';

        gh-jail-server = ghJailServerPkg;

        ax-jail = pkgs.writeShellScriptBin "ax-jail" ''
          export AX_JAIL_BIN_DIR="${axJailBinDir}/bin"
          export AX_JAIL_BWRAP="${pkgs.bubblewrap}/bin/bwrap"
          export NIX_SSL_CERT_FILE="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
          export NIX_CONFIG="extra-experimental-features = nix-command flakes"
          exec ${axJailUnwrapped}/bin/ax-jail "$@"
        '';

        codex-jail = pkgs.writeShellScriptBin "codex-jail" ''
          export CODEX_JAIL_BIN_DIR="${codexJailBinDir}/bin"
          export CODEX_JAIL_BWRAP="${pkgs.bubblewrap}/bin/bwrap"
          export CODEX_JAIL_GH_SERVER="${ghJailServerPkg}/bin/gh-jail-server"
          export PATH="${pkgs.gh}/bin:${pkgs.git}/bin:$PATH"
          export NIX_SSL_CERT_FILE="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
          export NIX_CONFIG="extra-experimental-features = nix-command flakes"
          exec ${codexJailUnwrapped}/bin/codex-jail "$@"
        '';
      };
  };
}
