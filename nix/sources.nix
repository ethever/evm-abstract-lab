# Cargo's workspace stays intact, but only a selected dependency closure contains
# real source. The other members have Crane-generated target stubs, so Cargo can
# still read every real manifest without an unrelated lib.rs invalidating a check.
{
  pkgs,
  craneLib,
  sourceRoot,
}:
let
  lib = pkgs.lib;
  rootManifest = builtins.fromTOML (builtins.readFile (sourceRoot + "/Cargo.toml"));
  normalize =
    path:
    lib.concatStringsSep "/" (
      lib.foldl' (
        parts: part:
        if part == "" || part == "." then
          parts
        else if part == ".." then
          lib.init parts
        else
          parts ++ [ part ]
      ) [ ] (lib.splitString "/" path)
    );
  relative = path: lib.removePrefix "${toString sourceRoot}/" (toString path);
  matches =
    pattern: value:
    builtins.match (lib.replaceStrings [ "\\*" "\\?" ] [ ".*" "." ] (lib.escapeRegex pattern)) value
    != null;
  cargoFiles = craneLib.findCargoFiles sourceRoot;
  manifests = lib.listToAttrs (
    map (
      path:
      let
        manifest = builtins.fromTOML (builtins.readFile path);
      in
      lib.nameValuePair (relative (dirOf path)) manifest
    ) (lib.filter (path: (builtins.fromTOML (builtins.readFile path)) ? package) cargoFiles.cargoTomls)
  );
  explicitMembers = lib.filter (
    path:
    lib.any (pattern: matches pattern path) rootManifest.workspace.members
    && !(lib.any (pattern: matches pattern path) (rootManifest.workspace.exclude or [ ]))
  ) (builtins.attrNames manifests);
  workspaceDependency =
    alias: value:
    if builtins.isAttrs value && (value.workspace or false) then
      let
        inherited =
          rootManifest.workspace.dependencies.${alias} or (throw "Unknown workspace dependency ${alias}");
        base = if builtins.isString inherited then { version = inherited; } else inherited;
      in
      base
      // (builtins.removeAttrs value [ "workspace" ])
      # Cargo adds member features to inherited workspace features.
      // lib.optionalAttrs (base ? features || value ? features) {
        features = lib.unique ((base.features or [ ]) ++ (value.features or [ ]));
      }
    else if builtins.isString value then
      { version = value; }
    else
      value;
  # Unknown target predicates are retained conservatively. These cover Cargo's
  # native/Wasm boundary; platform-specific predicates remain Cargo's decision.
  targetMatches =
    target: predicate:
    let
      compact = lib.replaceStrings [ " " "\t" "\n" ] [ "" "" "" ] predicate;
    in
    if compact == "cfg(target_arch=\"wasm32\")" || compact == "wasm32-unknown-unknown" then
      target == "wasm"
    else if compact == "cfg(not(target_arch=\"wasm32\"))" then
      target != "wasm"
    else
      true;
  tables =
    manifest: mode:
    let
      select =
        predicate: table:
        (lib.optional (table ? dependencies) {
          kind = "normal";
          inherit predicate;
          values = table.dependencies;
        })
        ++ (lib.optional (table ? build-dependencies) {
          kind = "build";
          inherit predicate;
          values = table.build-dependencies;
        })
        ++ (lib.optional (mode.kind == "test" && table ? dev-dependencies) {
          kind = "dev";
          inherit predicate;
          values = table.dev-dependencies;
        });
    in
    select null manifest
    ++ lib.concatLists (
      lib.mapAttrsToList (
        predicate: table: lib.optionals (targetMatches mode.target predicate) (select predicate table)
      ) (manifest.target or { })
    );
  dependenciesForPath =
    path: mode:
    lib.concatMap (
      table:
      lib.mapAttrsToList (
        alias: value:
        let
          spec = workspaceDependency alias value;
        in
        {
          inherit alias spec;
          inherit (table) kind predicate;
          path =
            if !(spec ? path) then
              null
            else if builtins.isAttrs value && (value.workspace or false) then
              normalize spec.path
            else
              normalize "${path}/${spec.path}";
        }
      ) table.values
    ) (tables manifests.${path} mode);
  # genericClosure, rather than a hand-maintained crate list, also follows new
  # path dependencies through arbitrary depth.
  members = builtins.genericClosure {
    startSet = map (path: { key = path; }) explicitMembers;
    operator =
      item:
      map (dependency: { key = dependency.path; }) (
        lib.filter (dependency: dependency.path != null) (
          (dependenciesForPath item.key {
            kind = "test";
            target = "native";
          })
          ++ (dependenciesForPath item.key {
            kind = "test";
            target = "wasm";
          })
        )
      );
  };
  packages = lib.listToAttrs (
    map (
      item:
      let
        path = item.key;
        manifest = manifests.${path};
      in
      lib.nameValuePair manifest.package.name { inherit path manifest; }
    ) members
  );
  names = builtins.attrNames packages;
  hasLibrary =
    name:
    let
      manifest = packages.${name}.manifest;
    in
    manifest ? lib
    || (
      (manifest.package.autolib or true)
      && builtins.pathExists (sourceRoot + "/${packages.${name}.path}/src/lib.rs")
    );
  hasDoctests = name: hasLibrary name && (packages.${name}.manifest.lib.doctest or true);
  hasDocs =
    name:
    let
      manifest = packages.${name}.manifest;
      bins = manifest.bin or [ ];
      implicitMain =
        (manifest.package.autobins or true)
        && builtins.pathExists (sourceRoot + "/${packages.${name}.path}/src/main.rs")
        && !(lib.any (bin: (bin.path or "") == "src/main.rs" || bin.name == name) bins);
      binDir = sourceRoot + "/${packages.${name}.path}/src/bin";
      implicitBins =
        (manifest.package.autobins or true)
        && builtins.pathExists binDir
        && lib.any (target: !(lib.any (bin: bin.name == lib.removeSuffix ".rs" target) bins)) (
          builtins.attrNames (builtins.readDir binDir)
        );
    in
    (hasLibrary name && (manifest.lib.doc or true))
    || lib.any (bin: bin.doc or true) bins
    || implicitMain
    || implicitBins;
  byPath = lib.listToAttrs (map (name: lib.nameValuePair packages.${name}.path name) names);
  dependencies =
    name: mode:
    map (
      dependency:
      dependency // { name = if dependency.path == null then null else byPath.${dependency.path}; }
    ) (dependenciesForPath packages.${name}.path mode);
  closure =
    roots: mode:
    map (item: item.key) (
      builtins.genericClosure {
        startSet = map (name: {
          key = name;
          root = true;
        }) roots;
        operator =
          item:
          map
            (dependency: {
              key = dependency.name;
              root = false;
            })
            (
              lib.filter (
                dependency:
                dependency.name != null
                && (
                  !(mode ? configuration)
                  || !(dependency.spec.optional or false)
                  || lib.elem dependency.alias mode.configuration.packages.${item.key}.dependencies
                )
              ) (dependencies item.key (mode // { kind = if item.root then mode.kind else "build"; }))
            );
      }
    );
  applicationFilter =
    path: type:
    let
      rel = relative path;
    in
    !(rel == "lints" || lib.hasPrefix "lints/" rel)
    # This corpus is read only by the independent Python scale check. Rust
    # fixtures use other examples subtrees; retaining it would invalidate every
    # test closure merely because a crate reads an unrelated example.
    && !(rel == "examples/no-smt-scale" || lib.hasPrefix "examples/no-smt-scale/" rel)
    && !(lib.elem (baseNameOf path) [
      ".git"
      "target"
      "node_modules"
      "result"
    ]);
  workspace = lib.cleanSourceWith {
    src = sourceRoot;
    name = "source";
    filter =
      path: type:
      applicationFilter path type
      && (
        craneLib.filterCargoSources path type
        || lib.hasPrefix "crates/" (relative path)
        || lib.hasPrefix "examples/" (relative path)
      );
  };
  formatting = lib.cleanSourceWith {
    src = sourceRoot;
    name = "rust-format-source";
    filter = path: type: applicationFilter path type && craneLib.filterCargoSources path type;
  };
  manifestsOnly = lib.cleanSourceWith {
    src = workspace;
    name = "workspace-manifests";
    filter =
      path: type:
      let
        paths = map relative (cargoFiles.cargoTomls ++ cargoFiles.cargoConfigs) ++ [ "Cargo.lock" ];
        rel = relative path;
      in
      if type == "directory" then
        lib.any (file: lib.hasPrefix "${rel}/" file) paths
      else
        lib.elem rel paths;
  };
  dummy = craneLib.mkDummySrc { src = workspace; };
  # Full crate directories preserve implicit targets, build.rs, native source,
  # include! data, fonts and future compile-time assets without extension lists.
  # Existing out-of-crate fixtures use computed paths, so retain their directory
  # whenever a selected crate refers to it (including test-only source).
  exampleReferences = lib.genAttrs names (
    name:
    map
      (
        path:
        let
          rel = relative path;
        in
        {
          development =
            lib.hasInfix "/tests/" rel
            || lib.hasSuffix "/tests.rs" rel
            || lib.hasInfix "/benches/" rel
            || lib.hasInfix "/examples/" rel;
          references = lib.hasInfix "examples/" (builtins.readFile path);
        }
      )
      (
        lib.filter (lib.hasSuffix ".rs") (
          lib.filesystem.listFilesRecursive (sourceRoot + "/${packages.${name}.path}")
        )
      )
  );
  # Nix functions do not memoize repeated arguments. Share file traversal and
  # content reads across every package closure, matrix alias and Cargo consumer.
  examplesByMode = lib.genAttrs [ "build" "test" ] (
    kind:
    lib.genAttrs names (
      name:
      lib.any (
        reference: (kind == "test" || !reference.development) && reference.references
      ) exampleReferences.${name}
    )
  );
  usesExamples = name: mode: examplesByMode.${mode.kind}.${name};
  sourceFor =
    roots: mode:
    let
      selected = closure roots mode;
      paths = map (name: packages.${name}.path) selected;
      examples = lib.any (name: usesExamples name mode) selected;
      real = lib.cleanSourceWith {
        src = sourceRoot;
        name = "selected-cargo-source";
        filter =
          path: type:
          let
            rel = relative path;
            developmentTarget =
              packagePath:
              lib.any
                (
                  directory: rel == "${packagePath}/${directory}" || lib.hasPrefix "${packagePath}/${directory}/" rel
                )
                [
                  "tests"
                  "benches"
                  "examples"
                ];
            # HTML is copied by web/assets.nix after the Wasm build. Keeping it
            # out of Rust inputs lets presentation edits reuse compiled assets.
          in
          applicationFilter path type
          && rel != "${packages.evm-abstract-web.path}/index.html"
          && (
            lib.any (
              packagePath:
              rel == packagePath
              || lib.hasPrefix "${packagePath}/" rel
              || (type == "directory" && lib.hasPrefix "${rel}/" packagePath)
            ) paths
            || (examples && (rel == "examples" || lib.hasPrefix "examples/" rel))
          )
          && (mode.kind == "test" || !(lib.any developmentTarget paths));
      };
    in
    pkgs.runCommand "source" { } ''
      mkdir -p "$out"
      cp -R ${dummy}/. "$out/"
      chmod -R u+w "$out"
      cp -R ${manifestsOnly}/. "$out/"
      cp -R ${real}/. "$out/"
    '';
in
{
  inherit
    rootManifest
    packages
    names
    byPath
    workspace
    formatting
    dummy
    sourceFor
    closure
    hasLibrary
    hasDoctests
    hasDocs
    dependencies
    workspaceDependency
    targetMatches
    tables
    ;
}
