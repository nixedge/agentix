let
  f = builtins.getFlake (toString ../../..);
  system = f.inputs.nixpkgs.lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      f.nixosModules.agentix
      ({pkgs, ...}: {
        nixpkgs.config.allowUnfree = true;
        boot.isContainer = true;
        fileSystems."/" = {
          device = "none";
          fsType = "tmpfs";
        };
        system.stateVersion = "26.05";
        services.agentix = {
          gpu = {
            enable = true;
            package = pkgs.hello;
            plans."qwen3.8-flash-next" = {
              peakBytes = 1024;
              modelSha256 = [(builtins.concatStringsSep "" (builtins.genList (_: "a") 64))];
              contextTokens = 4096;
              batchTokens = 256;
            };
          };
          qwen = {
            enable = true;
            package = pkgs.hello;
            shards = ["fixture.gguf"];
            expectedShardSha256 = ["fixture"];
          };
          llama = {
            enable = true;
            package = pkgs.hello;
          };
          whisper = {
            enable = true;
            package = pkgs.hello;
          };
        };
      })
    ];
  };
  c = system.config;
  names = ["agentix-qwen" "agentix-llama" "agentix-whisper"];
in {
  failedAssertions = map (a: a.message) (builtins.filter (a: !a.assertion) c.assertions);
  workers = builtins.listToAttrs (map (n: {
      name = n;
      value = {
        environment = c.systemd.services.${n}.environment;
        wants = c.systemd.services.${n}.wants;
      };
    })
    names);
  coordinator = c.systemd.services.agentix-gpu-coordinator.environment;
}
