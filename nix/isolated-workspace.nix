# Project a pinned Cargo workspace onto the dependency closure of selected roots.
{
  lib,
  pkgs,
  root,
  normalizeLock ? (candidate: candidate.scopedLock),
}: let
  readToml = path: builtins.fromTOML (builtins.readFile path);
  manifest = readToml (root + /Cargo.toml);
  lock = readToml (root + /Cargo.lock);
  members = lib.genAttrs manifest.workspace.members (member: readToml (root + "/${member}/Cargo.toml"));
  identity = package: "${package.name} ${package.version} (${package.source or ""})";
  byName = lib.groupBy (package: package.name) lock.package;
  resolve = dependency: let
    parts = lib.splitString " " dependency;
    candidates = lib.filter (package:
      (builtins.length parts < 2 || package.version == builtins.elemAt parts 1)
      && (builtins.length parts < 3 || "(${package.source or ""})" == lib.concatStringsSep " " (lib.drop 2 parts)))
    (byName.${builtins.head parts} or []);
  in
    if builtins.length candidates == 1
    then builtins.head candidates
    else throw "Ambiguous or missing Cargo.lock dependency: ${dependency}";
  memberFor = package: lib.findFirst (member: members.${member}.package.name == package.name) null (builtins.attrNames members);
  dependencySections = value:
    lib.concatLists (lib.mapAttrsToList (name: child:
      if builtins.elem name ["dependencies" "build-dependencies" "dev-dependencies"]
      then [child]
      else if builtins.isAttrs child
      then dependencySections child
      else [])
    value);
  toml = pkgs.formats.toml {};
  # Only the projected values enter generated derivations, not their source paths.
  detached = value: builtins.fromJSON (builtins.unsafeDiscardStringContext (builtins.toJSON value));
in {
  inherit members;
  scope = roots: let
    closure = builtins.genericClosure {
      startSet =
        map (name: let
          package = resolve "${members.${name}.package.name} ${members.${name}.package.version}";
        in {
          key = identity package;
          inherit package;
        })
        roots;
      operator = item:
        map (dependency: let
          package = resolve dependency;
        in {
          key = identity package;
          inherit package;
        }) (item.package.dependencies or []);
    };
    packages = map (item: item.package) closure;
    localPackages = lib.filter (package: !(package ? source)) packages;
    selectedMembers = lib.sort builtins.lessThan (lib.filter (member: member != null) (map memberFor localPackages));
    patchTables = lib.mapAttrs (_: patches:
      lib.filterAttrs (name: patch:
        lib.any (package: package.name == (patch.package or name)) localPackages)
      patches)
    (manifest.patch or {});
    selectedPatches = lib.filterAttrs (_: patches: patches != {}) patchTables;
    patchPaths = lib.concatLists (lib.mapAttrsToList (_: patches:
      lib.mapAttrsToList (_: patch: patch.path or (throw "Non-path patches require explicit isolation support")) patches)
    selectedPatches);
    inherited = lib.unique (lib.concatMap (member:
      lib.concatMap (section:
        builtins.attrNames (lib.filterAttrs (_: dependency:
          builtins.isAttrs dependency && dependency.workspace or false)
        section))
      (dependencySections members.${member}))
    selectedMembers);
    workspaceManifest = detached ((builtins.removeAttrs manifest ["patch" "workspace"])
      // {
        workspace =
          (builtins.removeAttrs manifest.workspace ["members" "default-members" "dependencies"])
          // {
            members = selectedMembers;
          }
          // lib.optionalAttrs (inherited != []) {
            dependencies = lib.getAttrs inherited manifest.workspace.dependencies;
          };
      }
      // lib.optionalAttrs (selectedPatches != {}) {patch = selectedPatches;});
    candidateLock = detached {
      inherit (lock) version;
      package = lib.sort (a: b: identity a < identity b) packages;
    };
    cargoToml = toml.generate "isolated-Cargo.toml" workspaceManifest;
    candidateCargoLock = toml.generate "candidate-Cargo.lock" candidateLock;
    scopedLock = detached (normalizeLock {
      scopedLock = candidateLock;
      src = makeSource manifestsSource stubCommands candidateCargoLock;
    });
    cargoLock = toml.generate "isolated-Cargo.lock" scopedLock;
    configFiles = lib.optional (builtins.pathExists (root + /.cargo)) (root + /.cargo);
    selectedPaths = map (member: root + "/${member}") selectedMembers ++ map (path: root + "/${path}") patchPaths;
    realSource = lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions (selectedPaths ++ configFiles);
    };
    manifestsSource = lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions (
        map (member: root + "/${member}/Cargo.toml") selectedMembers
        ++ map (path: root + "/${path}") patchPaths ++ configFiles
      );
    };
    stub = pkgs.writeText "workspace-target-stub.rs" "fn main() {}";
    stubCommands = lib.concatMapStringsSep "\n" (member: let
      memberManifest = members.${member};
      targets = lib.unique (
        lib.filter (path: builtins.pathExists (root + "/${member}/${path}")) ["src/lib.rs" "src/main.rs"]
        ++ map (bin: bin.path or "src/bin/${bin.name}.rs") (memberManifest.bin or [])
        ++ lib.optional (memberManifest ? lib) (memberManifest.lib.path or "src/lib.rs")
      );
    in
      lib.concatMapStringsSep "\n" (target: ''
        mkdir -p "$out/${member}/$(dirname ${lib.escapeShellArg target})"
        cp ${stub} "$out/${member}/${target}"
      '')
      targets)
    selectedMembers;
    makeSource = base: extra: lockFile:
      pkgs.runCommand "isolated-workspace-src" {} ''
        cp -rT ${base} $out
        chmod -R u+w $out
        cp ${cargoToml} $out/Cargo.toml
        cp ${lockFile} $out/Cargo.lock
        ${extra}
      '';
  in {
    inherit roots selectedMembers scopedLock cargoToml cargoLock;
    packageNames = map (package: package.name) scopedLock.package;
    src = makeSource realSource "" cargoLock;
    dependencySrc = makeSource manifestsSource stubCommands cargoLock;
    extraDummyScript =
      lib.concatMapStringsSep "\n" (path: ''
        cp -rT ${lib.fileset.toSource {
          inherit root;
          fileset = root + "/${path}";
        }}/${path} "$out/${path}"
      '')
      patchPaths;
  };
}
