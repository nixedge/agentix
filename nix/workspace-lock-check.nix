{
  pkgs,
  lib,
  toolchain,
  craneLib,
  scopes,
}: let
  checks = lib.mapAttrs (name: scope:
    pkgs.runCommand "${name}-locked-workspace" {
      nativeBuildInputs = [toolchain];
    } ''
      cp -rT ${scope.src} source
      chmod -R u+w source
      cd source
      export HOME="$TMPDIR/home"
      export CARGO_HOME="$TMPDIR/cargo"
      mkdir -p "$HOME" "$CARGO_HOME"
      cp ${(craneLib.vendorCargoDeps {cargoLockParsed = scope.scopedLock;})}/config.toml "$CARGO_HOME/config.toml"
      cargo metadata --locked --offline --all-features --format-version 1 > /dev/null
      touch "$out"
    '')
  scopes;
in
  pkgs.runCommand "workspace-locks" {} ''
    mkdir "$out"
    ${lib.concatStringsSep "\n" (lib.mapAttrsToList (name: check: "cp ${check} \"$out/${name}\"") checks)}
  ''
