# Two compatible caches follow the package DAG: registry-only dummy builds and
# real workspace libraries. Every consumer imports the latter before compiling
# its own targets, preserving fine invalidation without compiling core repeatedly.
{
  pkgs,
  craneLib,
  sources,
  common,
  commonFor,
  configurations,
  configuration,
  profile,
  mode,
  roots ? sources.names,
  libraryCommand ? (
    args: ''
      cargoWithProfile check ${args.cargoExtraArgs} --lib
      cargoWithProfile build ${args.cargoExtraArgs} --lib
    ''
  ),
  baseExtraArgs ? "",
  baseCommand ? null,
}:
let
  lib = pkgs.lib;
  contextConfiguration = configurations.forMode mode configuration;
  selected = sources.closure roots (mode // { configuration = contextConfiguration; });
  libraryProfile = configurations.libraryProfileFor profile;
  inherit (sources) hasLibrary;
  direct =
    name:
    lib.unique (
      map (dependency: dependency.name) (
        lib.filter (
          dependency:
          dependency.name != null
          && (
            !(dependency.spec.optional or false)
            || lib.elem dependency.alias contextConfiguration.packages.${name}.dependencies
          )
        ) (sources.dependencies name (mode // { kind = "build"; }))
      )
    );
  # Keeping target directories rather than repeatedly compressing them lets a
  # diamond-shaped graph share immutable .rlib/.rmeta files without extra work.
  merge =
    name: artifacts:
    let
      # Compare store paths rather than recursively comparing derivation attrs.
      # Preserve predecessor order so a real dependency remains authoritative.
      artifactKey = artifact: builtins.unsafeDiscardStringContext (toString artifact);
      byPath = lib.listToAttrs (
        map (artifact: lib.nameValuePair (artifactKey artifact) artifact) artifacts
      );
      inputs = map (path: byPath.${path}) (lib.unique (map artifactKey artifacts));
    in
    if builtins.length inputs == 1 then
      builtins.head inputs
    else
      pkgs.runCommand "${name}-cargo-artifacts" { nativeBuildInputs = [ pkgs.rsync ]; } ''
        mkdir -p "$out/target"
        ${lib.concatMapStringsSep "\n" (artifact: ''
          rsync -rlt --ignore-existing ${artifact}/target/ "$out/target/"
        '') inputs}
      '';
  # Dummy workspace outputs must never shadow a real predecessor's fingerprints.
  # Query Cargo for implicit as well as declared target names before discarding
  # only those local outputs; all registry/native dependency artifacts survive.
  discardDummyWorkspace = ''
    cargo metadata --offline --locked --no-deps --format-version 1 > "$TMPDIR/dummy-metadata.json"
    jq -r '.workspace_members as $members | .packages[] | select(.id as $id | $members | index($id)) | .name' \
      "$TMPDIR/dummy-metadata.json" > "$TMPDIR/dummy-packages"
    jq -r '.workspace_members as $members | .packages[] | select(.id as $id | $members | index($id)) | .targets[].name | gsub("-"; "_")' \
      "$TMPDIR/dummy-metadata.json" > "$TMPDIR/dummy-targets"
    while IFS= read -r package; do
      find target -type d \( -path "*/.fingerprint/$package-*" -o -path "*/build/$package-*" \) -prune -exec rm -rf {} +
    done < "$TMPDIR/dummy-packages"
    while IFS= read -r target; do
      find target \( -type f -o -type l \) \
        \( -path "*/deps/$target-*" -o -path "*/deps/lib$target-*" \
          -o -name "lib$target.rlib" -o -name "lib$target.rmeta" \) -delete
    done < "$TMPDIR/dummy-targets"
  '';
  scopeRoots = configurations.scopeRoots mode;
  base =
    let
      args = commonFor (builtins.head scopeRoots) mode contextConfiguration // {
        CARGO_PROFILE = profile;
        cargoExtraArgs = "--locked ${configurations.workspaceArgs mode contextConfiguration} ${baseExtraArgs}";
      };
    in
    craneLib.mkCargoDerivation (
      (builtins.removeAttrs args [ "src" ])
      // {
        pname = "${mode.target}-${mode.kind}-registry-artifacts";
        src = configurations.configuredDummyFor scopeRoots mode contextConfiguration;
        cargoArtifacts = null;
        nativeBuildInputs = args.nativeBuildInputs ++ [ pkgs.jq ];
        env.CRANE_BUILD_DEPS_ONLY = 1;
        installCargoArtifactsMode = "use-symlink";
        doCheck = mode.kind == "test";
        buildPhaseCargoCommand =
          if baseCommand != null then
            (if builtins.isFunction baseCommand then baseCommand args else baseCommand)
          else
            ''
              cargoWithProfile check ${args.cargoExtraArgs} ${
                lib.optionalString (mode.kind == "test") "--all-targets"
              }
              cargoWithProfile build ${args.cargoExtraArgs}
            '';
        checkPhaseCargoCommand = "cargoWithProfile test ${args.cargoExtraArgs} --all-targets --no-run";
        preInstall = discardDummyWorkspace;
      }
    );
  # One resolver context owns one third-party cache. Independent local branches
  # must not compile their shared serde/proptest/native dependencies separately.
  baseFor = lib.genAttrs selected (_: base);
  libraries = lib.genAttrs (lib.filter hasLibrary selected) (
    name:
    let
      args =
        commonFor name (
          mode
          // {
            sourceKind = if libraryProfile == "test" then "test" else "build";
          }
        ) contextConfiguration
        // {
          CARGO_PROFILE = libraryProfile;
          cargoArtifacts = merge "${name}-dependencies" (
            map (dependency: libraries.${dependency}) (lib.filter hasLibrary (direct name))
            ++ [ baseFor.${name} ]
          );
          installCargoArtifactsMode = "use-symlink";
        };
    in
    craneLib.mkCargoDerivation (
      args
      // {
        pname = "${name}-library-artifacts";
        doCheck = false;
        buildPhaseCargoCommand = libraryCommand args;
      }
    )
  );
  forPackage =
    name:
    let
      closure = sources.closure [ name ] (mode // { configuration = contextConfiguration; });
    in
    merge "${name}-selected" (
      map (dependency: libraries.${dependency}) (lib.filter hasLibrary closure) ++ [ baseFor.${name} ]
    );
in
{
  inherit
    base
    baseFor
    libraries
    forPackage
    merge
    libraryProfile
    ;
}
