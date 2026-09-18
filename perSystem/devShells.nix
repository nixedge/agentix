{inputs, ...}: {
  perSystem = {
    config,
    pkgs,
    inputs',
    ...
  }: let
    toolchain = inputs'.fenix.packages.stable.toolchain;
    cudaPackages = pkgs.cudaPackages_12;
    libcublasStatic = pkgs.lib.getOutput "static" cudaPackages.libcublas;
    # nvcc uses system("/bin/sh -c ...") internally, which fails on NixOS (no /bin/sh).
    # This wrapper runs cargo inside a bwrap namespace that provides /bin/sh → bash.
    cargoWrapper = pkgs.writeShellScript "cargo" ''
      exec ${pkgs.bubblewrap}/bin/bwrap \
        --dev-bind / / \
        --tmpfs /bin \
        --symlink ${pkgs.bash}/bin/bash /bin/sh \
        -- ${toolchain}/bin/cargo "$@"
    '';
    # Pinned GGUF fixture for integration tests (all-MiniLM-L6-v2, embedding-only, ~25 MB).
    # sha256 obtained via: nix store prefetch-file --json '<url>'
    fixture-model = pkgs.fetchurl {
      name = "all-MiniLM-L6-v2-Q8_0.gguf";
      url = "https://huggingface.co/second-state/All-MiniLM-L6-v2-Embedding-GGUF/resolve/main/all-MiniLM-L6-v2-Q8_0.gguf";
      sha256 = "sha256-JjIVw8rdbhZ0B0GnYkq0y7bI53doi9UzHs+/VoHC+O0=";
    };

    # Shared by both shells. The embedding backend (ollama vs llama-cpp) is the only
    # difference between them, so it is appended per shell rather than listed here.
    commonPackages = with pkgs; [
      # Rust toolchain (stable from fenix)
      toolchain
      pkg-config
      onnxruntime
      openssl
      # C/C++ toolchain required by llama-cpp-sys-2 (cmake build + bindgen)
      clang
      cmake
      ninja
      libclang.lib
      # PostgreSQL client (matches server version)
      postgresql_17
      # Python + uv for ingest and MCP server scripts
      uv
      # Utilities
      jq
      just
      # Formatter
      config.treefmt.build.wrapper
      # Lightweight hybrid BM25+semantic search CLI + MCP server (zero-config)
      inputs'.llm-agents.packages.ck
      # Claude Code usage analytics
      inputs'.llm-agents.packages.ccusage
      config.packages.ax-jail
      # CUDA toolkit for building agentix-daemon with --features cuda
      # cuda_cudart: libcudart_static.a is in the default output
      # libcublas: libcublas_static.a is in a separate 'static' output
      cudaPackages.cudatoolkit
      cudaPackages.cuda_cudart
      cudaPackages.libcublas
      libcublasStatic
    ];

    commonShellHook = ''
      export ORT_DYLIB_PATH="${pkgs.onnxruntime}/lib/libonnxruntime.so"
      export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
      export CMAKE_GENERATOR="Ninja"
      export CMAKE_MAKE_PROGRAM="${pkgs.ninja}/bin/ninja"
      export LD_LIBRARY_PATH="${pkgs.openssl.out}/lib:${pkgs.onnxruntime}/lib:${pkgs.libclang.lib}/lib:${pkgs.stdenv.cc.cc.lib}/lib:${cudaPackages.cuda_cudart}/lib:${cudaPackages.libcublas}/lib:$LD_LIBRARY_PATH"
      export CUDA_HOME="${cudaPackages.cudatoolkit}"
      export CUDA_PATH="${cudaPackages.cudatoolkit}"
      export CMAKE_CUDA_COMPILER="${cudaPackages.cudatoolkit}/bin/nvcc"
      # Static CUDA lib search paths for llama-cpp-sys-2.
      # cublas/cudart have a separate 'static' output in nixpkgs (not in the default 'lib' output).
      # lib/stubs contains libcuda.so — the driver API stub needed at link time.
      export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }-L ${cudaPackages.cudatoolkit}/lib -L ${cudaPackages.cudatoolkit}/lib/stubs -L ${cudaPackages.cuda_cudart}/lib -L ${libcublasStatic}/lib"
      # nvcc uses system("/bin/sh -c ...") internally, but NixOS has no /bin/sh.
      # Prepend a cargo wrapper that runs the build inside a bwrap namespace with /bin/sh.
      _CARGO_WRAP_DIR=$(mktemp -d)
      ln -s ${cargoWrapper} "$_CARGO_WRAP_DIR/cargo"
      export PATH="$_CARGO_WRAP_DIR:$PATH"
      # Fixture GGUF for integration tests — pinned FOD derivation, no network needed
      export AGENTIX_TEST_MODEL_PATH="${fixture-model}"
    '';
  in {
    devShells.default = pkgs.mkShell {
      packages = commonPackages ++ [pkgs.ollama];
      shellHook =
        commonShellHook
        + ''
          echo "Agentic RAG Stack"
          echo ""
          echo "Services:"
          echo "  nix run .#dev              # Start PostgreSQL (ParadeDB) + Ollama"
          echo ""
          echo "Indexing:"
          echo "  just index /path/to/repo   # Index a codebase"
          echo ""
          echo "Database:"
          echo "  psql postgres://127.0.0.1:5432/codebase"
          echo ""
          echo "MCP server:"
          echo "  just build                 # Build the Rust binary"
          echo "  just mcp                   # Run the MCP server"
          echo "  nix build .#agentix-mcp-server  # Build with Nix (requires Cargo.lock)"
          echo ""
          echo "Quick search (no services needed):"
          echo "  ck search 'query'              # ad-hoc hybrid search"
          echo "  ck --serve                     # lightweight MCP server"
          echo "  ccusage                        # Claude Code usage stats"
        '';
    };

    devShells.llama-cpp = pkgs.mkShell {
      packages = commonPackages ++ [pkgs.llama-cpp];
      shellHook =
        commonShellHook
        + ''
          export LLAMACPP_HOST="http://127.0.0.1:8080"
          echo "Agentic RAG Stack (llama.cpp)"
          echo ""
          echo "Services:"
          echo "  nix run .#dev-llama        # Start PostgreSQL (ParadeDB) + llama-server"
          echo ""
          echo "Indexing:"
          echo "  just index /path/to/repo   # Index a codebase"
          echo ""
          echo "Database:"
          echo "  psql postgres://127.0.0.1:5432/codebase"
          echo ""
          echo "MCP server:"
          echo "  just build                 # Build the Rust binary"
          echo "  just mcp                   # Run the MCP server"
          echo "  nix build .#agentix-mcp-server  # Build with Nix (requires Cargo.lock)"
          echo ""
          echo "Quick search (no services needed):"
          echo "  ck search 'query'              # ad-hoc hybrid search"
          echo "  ck --serve                     # lightweight MCP server"
          echo "  ccusage                        # Claude Code usage stats"
        '';
    };
  };
}
