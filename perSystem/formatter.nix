{
  perSystem = {pkgs, ...}: {
    treefmt = {
      projectRootFile = "flake.nix";
      settings.global.excludes = ["patches/rusting-strata/**"];
      programs.rustfmt.enable = true;
      programs.alejandra.enable = true;
    };
  };
}
