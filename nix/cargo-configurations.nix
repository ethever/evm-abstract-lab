# Resolve local features inside the original Cargo root/target context. Cargo
# itself resolves registry features through a package with a disabled target.
{
  pkgs,
  sources,
  craneLib ? null,
  cargoVendorDir ? null,
}:
let
  lib = pkgs.lib;
  sorted = values: lib.sort builtins.lessThan (lib.unique values);
  forModes =
    f:
    lib.genAttrs [ "native" "wasm" ] (
      target: lib.genAttrs [ "build" "test" ] (kind: f { inherit target kind; })
    );
  defaultRoots =
    mode:
    if mode.target == "wasm" && sources.packages ? evm-abstract-web then
      [ "evm-abstract-web" ]
    else
      sources.names;
  scopeRoots = mode: mode.featureRoots or (defaultRoots mode);
  dependenciesByName = lib.genAttrs sources.names (
    name:
    lib.unique (
      (sources.dependencies name {
        kind = "test";
        target = "native";
      })
      ++ (sources.dependencies name {
        kind = "test";
        target = "wasm";
      })
    )
  );
  definitions = lib.genAttrs sources.names (
    name:
    let
      declared = sources.packages.${name}.manifest.features or { };
      suppressed = lib.concatMap (
        values: map (lib.removePrefix "dep:") (lib.filter (lib.hasPrefix "dep:") values)
      ) (builtins.attrValues declared);
      implicit = lib.listToAttrs (
        lib.concatMap (
          dependency:
          lib.optional ((dependency.spec.optional or false) && !(lib.elem dependency.alias suppressed)) (
            lib.nameValuePair dependency.alias [ "dep:${dependency.alias}" ]
          )
        ) dependenciesByName.${name}
      );
    in
    implicit // declared
  );
  empty = lib.genAttrs sources.names (_: {
    active = false;
    features = [ ];
    dependencies = [ ];
    requests = { };
  });
  activate =
    state: name:
    state
    // {
      ${name} = state.${name} // {
        active = true;
      };
    };
  addFeature =
    state: name: feature:
    if !(builtins.hasAttr feature definitions.${name}) then
      state
    else
      state
      // {
        ${name} = state.${name} // {
          features = sorted (state.${name}.features ++ [ feature ]);
        };
      };
  addDependency =
    state: name: alias:
    state
    // {
      ${name} = state.${name} // {
        dependencies = sorted (state.${name}.dependencies ++ [ alias ]);
      };
    };
  addRequest =
    state: name: alias: feature:
    state
    // {
      ${name} = state.${name} // {
        requests = state.${name}.requests // {
          ${alias} = sorted ((state.${name}.requests.${alias} or [ ]) ++ [ feature ]);
        };
      };
    };
  expandValue =
    name: state: value:
    if lib.hasPrefix "dep:" value then
      addDependency state name (lib.removePrefix "dep:" value)
    else if lib.hasInfix "/" value then
      let
        parts = lib.splitString "/" value;
        raw = builtins.head parts;
        alias = lib.removeSuffix "?" raw;
      in
      if lib.hasSuffix "?" raw && !(lib.elem alias state.${name}.dependencies) then
        state
      else
        addRequest (addDependency state name alias) name alias (lib.last parts)
    else
      addFeature state name value;
  expandPackage =
    mode: roots: state: name:
    if !state.${name}.active then
      state
    else
      let
        dependencies = sources.dependencies name (
          mode
          // {
            kind = if lib.elem name roots then mode.kind else "build";
          }
        );
        seeded = lib.foldl' (
          current: dependency:
          if dependency.spec.optional or false then current else addDependency current name dependency.alias
        ) state dependencies;
        expanded = lib.foldl' (
          current: feature: lib.foldl' (expandValue name) current definitions.${name}.${feature}
        ) seeded seeded.${name}.features;
      in
      lib.foldl' (
        current: dependency:
        if dependency.name == null || !(lib.elem dependency.alias current.${name}.dependencies) then
          current
        else
          lib.foldl' (next: feature: addFeature next dependency.name feature)
            (activate current dependency.name)
            (
              (dependency.spec.features or [ ])
              ++ (current.${name}.requests.${dependency.alias} or [ ])
              ++ lib.optional (dependency.spec.default-features or true) "default"
            )
      ) expanded dependencies;
  fixedPoint =
    mode: roots: state:
    let
      next = lib.foldl' (expandPackage mode roots) state sources.names;
    in
    if next == state then state else fixedPoint mode roots next;
  resolve =
    selection: mode:
    let
      roots = scopeRoots mode;
      seeded = lib.foldl' (
        state: name:
        lib.foldl' (current: feature: addFeature current name feature) (activate state name) (
          [ "default" ]
          ++ (
            if selection == "all" then
              builtins.attrNames definitions.${name}
            else
              lib.optional (selection == "imbl") "imbl"
          )
        )
      ) empty roots;
      packages = fixedPoint mode roots seeded;
    in
    {
      inherit packages selection;
      signature = builtins.toJSON packages;
    };
  resolvedByMode = forModes (
    mode: lib.genAttrs [ "default" "imbl" "all" ] (selection: resolve selection mode)
  );
  forMode =
    mode: configuration:
    if scopeRoots mode == defaultRoots mode then
      resolvedByMode.${mode.target}.${mode.kind}.${configuration.selection}
    else
      resolve configuration.selection mode;
  default = resolvedByMode.native.test.default;
  imbl = resolvedByMode.native.test.imbl;
  allFeatures = resolvedByMode.native.test.all;
  configurationsByMode = forModes (
    mode:
    lib.listToAttrs (
      map (configuration: lib.nameValuePair configuration.signature configuration) (
        builtins.attrValues resolvedByMode.${mode.target}.${mode.kind}
      )
    )
  );
  devProfile = sources.rootManifest.profile.dev or { };
  testProfile = lib.recursiveUpdate devProfile (
    builtins.removeAttrs (sources.rootManifest.profile.test or { }) [ "panic" ]
  );
  testSharesDevArtifacts = testProfile == devProfile && (devProfile.panic or "unwind") == "unwind";
  # --profile test selects a harness even for build --lib. Equal declared settings
  # permit ordinary dev libraries; changed test settings select a separate cache.
  libraryProfileFor = profile: if profile == "test" && testSharesDevArtifacts then "dev" else profile;
  unifier = import ./cargo-feature-unifier.nix {
    inherit
      pkgs
      sources
      craneLib
      cargoVendorDir
      ;
  };
  unifiers = forModes (
    mode:
    lib.mapAttrs (
      _: configuration: unifier { inherit mode configuration; }
    ) configurationsByMode.${mode.target}.${mode.kind}
  );
  contextFor =
    mode: configuration:
    let
      selected = forMode mode configuration;
    in
    if scopeRoots mode == defaultRoots mode then
      unifiers.${mode.target}.${mode.kind}.${selected.signature}
    else
      unifier {
        inherit mode;
        configuration = selected;
      };
  resolverManifest = mode: configuration: (contextFor mode configuration).manifest;
  computePatchedManifest =
    name: mode: configuration:
    let
      patchTable =
        kind: values:
        lib.mapAttrs (
          alias: original:
          let
            spec = sources.workspaceDependency alias original;
            dependency = lib.findFirst (candidate: candidate.alias == alias && candidate.kind == kind) null (
              sources.dependencies name mode
            );
          in
          if dependency == null || dependency.name == null then
            original
          else
            original
            // {
              features = sorted ((spec.features or [ ]) ++ configuration.packages.${dependency.name}.features);
            }
        ) values;
      patch =
        table:
        table
        // lib.optionalAttrs (table ? dependencies) {
          dependencies = patchTable "normal" table.dependencies;
        }
        // lib.optionalAttrs (table ? build-dependencies) {
          build-dependencies = patchTable "build" table.build-dependencies;
        }
        // lib.optionalAttrs (table ? dev-dependencies) {
          dev-dependencies = patchTable "dev" table.dev-dependencies;
        };
      manifest = sources.packages.${name}.manifest;
    in
    patch manifest
    // lib.optionalAttrs (manifest ? target) {
      target = lib.mapAttrs (
        predicate: table: if sources.targetMatches mode.target predicate then patch table else table
      ) manifest.target;
    };
  patchedManifests = forModes (
    mode:
    lib.mapAttrs (
      _: configuration: lib.genAttrs sources.names (name: computePatchedManifest name mode configuration)
    ) configurationsByMode.${mode.target}.${mode.kind}
  );
  patchManifest =
    name: mode: configuration:
    let
      selected = forMode mode configuration;
    in
    if scopeRoots mode == defaultRoots mode then
      patchedManifests.${mode.target}.${mode.kind}.${selected.signature}.${name}
    else
      computePatchedManifest name mode selected;
  featureArgs =
    name: configuration:
    lib.optionalString (configuration.packages.${name}.features != [ ])
      "--features ${
        lib.escapeShellArg (lib.concatStringsSep "," configuration.packages.${name}.features)
      }";
  packageArgs =
    name: mode: configuration:
    let
      selected = forMode mode configuration;
      features = map (feature: "${name}/${feature}") selected.packages.${name}.features;
    in
    "-p ${lib.escapeShellArg name} -p nix-feature-unifier --no-default-features"
    + lib.optionalString (
      features != [ ]
    ) " --features ${lib.escapeShellArg (lib.concatStringsSep "," features)}";
  workspaceFeatureArgs =
    configuration:
    lib.concatMapStringsSep " " (
      name:
      lib.concatMapStringsSep " " (
        feature: "--features ${lib.escapeShellArg "${name}/${feature}"}"
      ) configuration.packages.${name}.features
    ) sources.names;
  workspaceArgs =
    mode: configuration:
    let
      selected = forMode mode configuration;
    in
    if mode.target == "wasm" then
      packageArgs "evm-abstract-web" mode selected
    else
      "--workspace --no-default-features ${workspaceFeatureArgs selected}";
  writeTOML = name: value: (pkgs.formats.toml { }).generate name value;
  dummyBuild = pkgs.writeText "dummy-build.rs" "fn main() {}\n";
  manifestFiles = forModes (
    mode:
    lib.mapAttrs (
      _: configuration:
      lib.genAttrs sources.names (
        name:
        let
          manifest = patchManifest name mode configuration;
        in
        {
          real = writeTOML "${name}-Cargo.toml" manifest;
          dummy = writeTOML "${name}-Cargo.toml" (
            manifest
            // {
              package = manifest.package // {
                build = dummyBuild;
              };
            }
          );
        }
      )
    ) configurationsByMode.${mode.target}.${mode.kind}
  );
  makeSource =
    dummy: roots: mode: requested:
    let
      configuration = forMode mode requested;
      sourceMode = mode // {
        kind = mode.sourceKind or mode.kind;
        inherit configuration;
      };
      selected = sources.closure roots sourceMode;
      files = map (name: {
        path = sources.packages.${name}.path;
        file =
          manifestFiles.${mode.target}.${mode.kind}.${configuration.signature}.${name}.${
            if dummy then "dummy" else "real"
          };
      }) selected;
      base = if dummy then sources.dummy else sources.sourceFor roots sourceMode;
    in
    pkgs.runCommand "source" { } ''
      mkdir -p "$out"
      cp -R ${base}/. "$out/"
      chmod -R u+w "$out"
      cp -R ${(contextFor mode configuration).overlay}/. "$out/"
      ${lib.concatMapStringsSep "\n" (
        manifest: ''cp ${manifest.file} "$out/${manifest.path}/Cargo.toml"''
      ) files}
    '';
  # Shared attrsets memoize source traversal/TOML generation across all Cargo
  # consumers. Matrix names with equal resolved state select the same entries.
  sourceCache = forModes (
    mode:
    lib.genAttrs [ "build" "test" ] (
      sourceKind:
      lib.mapAttrs (
        _: configuration:
        lib.genAttrs sources.names (
          name: makeSource false [ name ] (mode // { inherit sourceKind; }) configuration
        )
      ) configurationsByMode.${mode.target}.${mode.kind}
    )
  );
  dummyCache = forModes (
    mode:
    lib.mapAttrs (
      _: configuration: lib.genAttrs sources.names (name: makeSource true [ name ] mode configuration)
    ) configurationsByMode.${mode.target}.${mode.kind}
  );
  configuredSource =
    roots: mode: configuration:
    let
      selected = forMode mode configuration;
    in
    if builtins.length roots == 1 && scopeRoots mode == defaultRoots mode then
      sourceCache.${mode.target}.${mode.kind}.${mode.sourceKind or mode.kind}.${selected.signature}.${builtins.head roots}
    else
      makeSource false roots mode selected;
  configuredDummyFor =
    roots: mode: configuration:
    let
      selected = forMode mode configuration;
    in
    if builtins.length roots == 1 && scopeRoots mode == defaultRoots mode then
      dummyCache.${mode.target}.${mode.kind}.${selected.signature}.${builtins.head roots}
    else
      makeSource true roots mode selected;
in
{
  inherit
    default
    imbl
    allFeatures
    forMode
    scopeRoots
    resolverManifest
    contextFor
    packageArgs
    workspaceArgs
    featureArgs
    workspaceFeatureArgs
    patchManifest
    configuredSource
    configuredDummyFor
    testSharesDevArtifacts
    libraryProfileFor
    ;
  configuredDummy = configuredDummyFor sources.names;
  unique = builtins.attrValues configurationsByMode.native.test;
}
