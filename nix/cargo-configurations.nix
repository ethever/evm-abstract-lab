# Resolve workspace feature requests before splitting Cargo invocations. Cargo's
# resolver still owns registry dependencies; this fixed point makes each local
# package see the features it saw in the original --workspace matrix.
{ pkgs, sources }:
let
  lib = pkgs.lib;
  sorted = values: lib.sort builtins.lessThan (lib.unique values);
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
  allDependencies = name: dependenciesByName.${name};
  featureDefinitions =
    name:
    let
      declared = sources.packages.${name}.manifest.features or { };
      suppressed = lib.concatMap (
        values: map (value: lib.removePrefix "dep:" value) (lib.filter (lib.hasPrefix "dep:") values)
      ) (builtins.attrValues declared);
      implicit = lib.listToAttrs (
        lib.concatMap (
          dependency:
          lib.optional ((dependency.spec.optional or false) && !(lib.elem dependency.alias suppressed)) (
            lib.nameValuePair dependency.alias [ "dep:${dependency.alias}" ]
          )
        ) (allDependencies name)
      );
    in
    implicit // declared;
  definitions = lib.genAttrs sources.names featureDefinitions;
  empty = lib.genAttrs sources.names (_: {
    features = [ ];
    dependencies = [ ];
    requests = { };
  });
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
        rawAlias = builtins.head parts;
        weak = lib.hasSuffix "?" rawAlias;
        alias = lib.removeSuffix "?" rawAlias;
        feature = lib.last parts;
        enabled = lib.elem alias state.${name}.dependencies;
      in
      if weak && !enabled then state else addRequest (addDependency state name alias) name alias feature
    else
      addFeature state name value;
  expandPackage =
    state: name:
    let
      seeded = lib.foldl' (
        current: dependency:
        if dependency.spec.optional or false then current else addDependency current name dependency.alias
      ) state (allDependencies name);
      expanded = lib.foldl' (
        current: feature: lib.foldl' (expandValue name) current definitions.${name}.${feature}
      ) seeded seeded.${name}.features;
    in
    lib.foldl' (
      current: dependency:
      if dependency.name == null || !(lib.elem dependency.alias current.${name}.dependencies) then
        current
      else
        lib.foldl' (next: feature: addFeature next dependency.name feature) current (
          (dependency.spec.features or [ ])
          ++ (current.${name}.requests.${dependency.alias} or [ ])
          ++ lib.optional (dependency.spec.default-features or true) "default"
        )
    ) expanded (allDependencies name);
  fixedPoint =
    state:
    let
      next = lib.foldl' expandPackage state sources.names;
    in
    if next == state then state else fixedPoint next;
  resolve =
    selection:
    let
      seeded = lib.foldl' (
        state: name:
        lib.foldl' (current: feature: addFeature current name feature) state (
          [ "default" ]
          ++ (
            if selection == "all" then
              builtins.attrNames definitions.${name}
            else
              lib.optional (selection == "imbl") "imbl"
          )
        )
      ) empty sources.names;
      packages = fixedPoint seeded;
    in
    {
      inherit packages;
      signature = builtins.toJSON packages;
    };
  default = resolve "default";
  imblCandidate = resolve "imbl";
  imbl = if imblCandidate.signature == default.signature then default else imblCandidate;
  allCandidate = resolve "all";
  allFeatures =
    if allCandidate.signature == default.signature then
      default
    else if allCandidate.signature == imbl.signature then
      imbl
    else
      allCandidate;
  devProfile = sources.rootManifest.profile.dev or { };
  testProfile = lib.recursiveUpdate devProfile (
    builtins.removeAttrs (sources.rootManifest.profile.test or { }) [ "panic" ]
  );
  # --profile test selects lib test harnesses even for `cargo build --lib`.
  # Only use ordinary dev libraries when their actual declared settings match;
  # a future test-only profile change must select a separate cache automatically.
  testSharesDevArtifacts = testProfile == devProfile && (devProfile.panic or "unwind") == "unwind";
  libraryProfileFor = profile: if profile == "test" && testSharesDevArtifacts then "dev" else profile;
  configurationsBySignature = lib.listToAttrs (
    map (configuration: lib.nameValuePair configuration.signature configuration) [
      default
      imbl
      allFeatures
    ]
  );
  forModes =
    function:
    lib.genAttrs [ "native" "wasm" ] (
      target: lib.genAttrs [ "build" "test" ] (kind: function { inherit target kind; })
    );
  # Resolver 2/3 separates host build-dependency features from normal/dev ones.
  # Keep those groups separate while reproducing workspace-wide unification.
  dependencyKey =
    dependency:
    builtins.toJSON {
      name = dependency.spec.package or dependency.alias;
      source = builtins.removeAttrs dependency.spec [
        "features"
        "default-features"
        "optional"
        "package"
      ];
      host = dependency.kind == "build";
    };
  computeRegistryRequests =
    mode: configuration:
    lib.foldl' (
      groups: name:
      lib.foldl' (
        current: dependency:
        if
          dependency.name != null || !(lib.elem dependency.alias configuration.packages.${name}.dependencies)
        then
          current
        else
          let
            key = dependencyKey dependency;
            previous =
              current.${key} or {
                features = [ ];
                defaults = false;
              };
          in
          current
          // {
            ${key} = {
              features = sorted (
                previous.features
                ++ (dependency.spec.features or [ ])
                ++ (configuration.packages.${name}.requests.${dependency.alias} or [ ])
              );
              defaults = previous.defaults || (dependency.spec.default-features or true);
            };
          }
      ) groups (sources.dependencies name mode)
    ) { } sources.names;
  registryRequestsByMode = forModes (
    mode:
    lib.mapAttrs (
      _: configuration: computeRegistryRequests mode configuration
    ) configurationsBySignature
  );
  registryRequests =
    mode: configuration: registryRequestsByMode.${mode.target}.${mode.kind}.${configuration.signature};
  computePatchedManifest =
    name: mode: configuration:
    let
      groups = registryRequests mode configuration;
      patchTable =
        kind: values:
        lib.mapAttrs (
          alias: original:
          let
            spec = sources.workspaceDependency alias original;
            dependency = lib.findFirst (candidate: candidate.alias == alias && candidate.kind == kind) null (
              sources.dependencies name mode
            );
            local = dependency != null && dependency.name != null;
            request =
              if local then
                {
                  features = configuration.packages.${dependency.name}.features;
                  defaults = spec.default-features or true;
                }
              else if dependency != null then
                groups.${dependencyKey dependency} or {
                  features = [ ];
                  defaults = spec.default-features or true;
                }
              else
                {
                  features = [ ];
                  defaults = spec.default-features or true;
                };
            # Inherited paths are rooted at the workspace manifest, whereas an
            # explicit dependency's path is relative to its package manifest.
            base =
              if (builtins.isAttrs original && (original.workspace or false)) then
                original
              else if builtins.isString original then
                { version = original; }
              else
                original;
          in
          base
          // {
            features = sorted ((spec.features or [ ]) ++ request.features);
            default-features = request.defaults;
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
    ) configurationsBySignature
  );
  patchManifest =
    name: mode: configuration:
    patchedManifests.${mode.target}.${mode.kind}.${configuration.signature}.${name};
  featureArgs =
    name: configuration:
    let
      features = configuration.packages.${name}.features;
    in
    lib.optionalString (
      features != [ ]
    ) "--features ${lib.escapeShellArg (lib.concatStringsSep "," features)}";
  # Only manifests in this closure are rewritten. An imbl-only matrix change
  # must not alter the notation/protocol derivation hashes.
  makeConfiguredSource =
    roots: mode: configuration:
    let
      sourceMode = mode // {
        kind = mode.sourceKind or mode.kind;
        inherit configuration;
      };
      selected = sources.closure roots sourceMode;
      manifests = map (name: {
        path = sources.packages.${name}.path;
        file = manifestFiles.${mode.target}.${mode.kind}.${configuration.signature}.${name}.real;
      }) selected;
      base = sources.sourceFor roots sourceMode;
    in
    pkgs.runCommand "source" { } ''
      mkdir -p "$out"
      cp -R ${base}/. "$out/"
      chmod -R u+w "$out"
      ${lib.concatMapStringsSep "\n" (
        manifest: ''cp ${manifest.file} "$out/${manifest.path}/Cargo.toml"''
      ) manifests}
    '';
  # Crane's writer is exposed by its library and keeps generated TOML stable.
  craneWriteTOML = name: value: (pkgs.formats.toml { }).generate name value;
  dummyBuild = pkgs.writeText "dummy-build.rs" "fn main() {}\n";
  dummyRoot = craneWriteTOML "Cargo.toml" sources.rootManifest;
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
          real = craneWriteTOML "${name}-Cargo.toml" manifest;
          dummy = craneWriteTOML "${name}-Cargo.toml" (
            manifest
            // {
              package = manifest.package // {
                build = dummyBuild;
              };
            }
          );
        }
      )
    ) configurationsBySignature
  );
  # Materialize each single-package source once. Its dependency closure can be
  # large, and tests, Clippy, Rustdoc and library caches all request the same tree.
  configuredSources = forModes (
    mode:
    lib.genAttrs [ "build" "test" ] (
      sourceKind:
      lib.mapAttrs (
        _: configuration:
        lib.genAttrs sources.names (
          name: makeConfiguredSource [ name ] (mode // { inherit sourceKind; }) configuration
        )
      ) configurationsBySignature
    )
  );
  configuredSource =
    roots: mode: configuration:
    if builtins.length roots == 1 then
      configuredSources.${mode.target}.${mode.kind}.${mode.sourceKind or mode.kind}.${configuration.signature}.${builtins.head roots}
    else
      makeConfiguredSource roots mode configuration;
  makeConfiguredDummy =
    roots: mode: configuration:
    let
      files = map (name: {
        path = sources.packages.${name}.path;
        file = manifestFiles.${mode.target}.${mode.kind}.${configuration.signature}.${name}.dummy;
      }) (sources.closure roots (mode // { inherit configuration; }));
    in
    pkgs.runCommand "source" { } ''
      mkdir -p "$out"
      cp -R ${sources.dummy}/. "$out/"
      chmod -R u+w "$out"
      cp ${dummyRoot} "$out/Cargo.toml"
      ${lib.concatMapStringsSep "\n" (
        manifest: ''cp ${manifest.file} "$out/${manifest.path}/Cargo.toml"''
      ) files}
    '';
  configuredDummies = forModes (
    mode:
    lib.mapAttrs (
      _: configuration: lib.genAttrs sources.names (name: makeConfiguredDummy [ name ] mode configuration)
    ) configurationsBySignature
  );
  configuredDummyFor =
    roots: mode: configuration:
    if builtins.length roots == 1 then
      configuredDummies.${mode.target}.${mode.kind}.${configuration.signature}.${builtins.head roots}
    else
      makeConfiguredDummy roots mode configuration;
  configuredDummy = configuredDummyFor sources.names;
  workspaceFeatureArgs =
    configuration:
    lib.concatMapStringsSep " " (
      name:
      lib.concatMapStringsSep " " (
        feature: "--features ${lib.escapeShellArg "${name}/${feature}"}"
      ) configuration.packages.${name}.features
    ) sources.names;
in
{
  inherit
    default
    imbl
    allFeatures
    featureArgs
    configuredSource
    configuredDummy
    configuredDummyFor
    workspaceFeatureArgs
    patchManifest
    testSharesDevArtifacts
    libraryProfileFor
    ;
  unique = lib.unique [
    default
    imbl
    allFeatures
  ];
}
