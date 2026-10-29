# Binary variant evaluation fix

Kursk deployment evaluation exposed a missing `old.cargoExtraArgs` attribute in the store-server package override. The five jail variants used the same pattern. Crane consumes `cargoExtraArgs` when constructing the generated build command, so it is unavailable through `overrideAttrs`; adding a fallback would also leave the actual build command unchanged.

Build member variants with their package/binary arguments before calling `craneLib.buildPackage`. They retain their existing scoped sources and dependency artifacts. Existing member packages use the same shared builder without extra overrides.

Verification:

- Reproduced the original store-server derivation evaluation error.
- Every `packages.x86_64-linux` output now evaluates to a derivation path.
- `nix build .#checks.x86_64-linux.package-binary-selection --no-link` passes. It checks the final generated build phase for the store-server and all five jail binaries, including the renamed gh client.
- Local NixOS module evaluation forces both the storage and assistant service commands successfully.
- Alejandra and whitespace checks pass.

This verifies package and local service evaluation. It does not evaluate Kursk's full host configuration, compile every variant again, or deploy anything.
