{...}: {
  perSystem = {config, ...}: let
    checked = config.packages.agentix-ourgroceries.overrideAttrs (_: {
      doCheck = true;
      checkPhase = ''
        runHook preCheck
        cargo test --release --locked --offline --package agentix-ourgroceries
        runHook postCheck
      '';
    });
  in {
    packages.ourgroceries = checked;
    checks.ourgroceries = checked;
  };
}
