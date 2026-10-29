{
  pkgs,
  craneLib,
  toolchain,
}: candidate: let
  vendor = craneLib.vendorCargoDeps {cargoLockParsed = candidate.scopedLock;};
  pins = pkgs.writeText "allowed-cargo-pins.json" (builtins.toJSON candidate.scopedLock);
  normalized =
    pkgs.runCommand "normalized-Cargo.lock" {
      nativeBuildInputs = [toolchain pkgs.remarshal pkgs.jq];
    } ''
      cp -rT ${candidate.src} source
      chmod -R u+w source
      cd source
      export HOME="$TMPDIR/home"
      export CARGO_HOME="$TMPDIR/cargo"
      mkdir -p "$HOME" "$CARGO_HOME"
      cp ${vendor}/config.toml "$CARGO_HOME/config.toml"
      cargo metadata --offline --format-version 1 > /dev/null
      remarshal -i Cargo.lock -if toml -of json -o normalized.json
      jq -e --slurpfile allowed ${pins} '
        all(.package[]; . as $p |
          any($allowed[0].package[];
            .name == $p.name and .version == $p.version and
            .source == $p.source and .checksum == $p.checksum))
      ' normalized.json > /dev/null
      cargo metadata --locked --offline --format-version 1 > /dev/null
      cp Cargo.lock "$out"
    '';
in
  # The normalized content, rather than its preparation derivation, defines the
  # downstream cache key. Cargo's workspace-wide feature edges can be unrelated.
  builtins.fromTOML (builtins.readFile normalized)
