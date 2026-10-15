# Wasm caches inherit the Web root's registry and local feature requests.
# Unrelated native members must not change that context or its cache identity.
{ pkgs }:
let
  lib = pkgs.lib;
  names = [
    "evm-abstract-web"
    "renderer"
    "native-tool"
  ];
  make =
    nativeFeature:
    let
      packages = {
        evm-abstract-web = {
          path = "crates/evm-abstract-web";
          manifest = {
            package.name = "evm-abstract-web";
            dependencies = {
              renderer.path = "../renderer";
              web-lib = {
                version = "=1.0.0";
                features = [ "browser" ];
              };
            };
            dev-dependencies.dev-only.version = "=1.0.0";
            target."cfg(not(target_arch = \"wasm32\"))".dependencies.native-target.version = "=1.0.0";
          };
        };
        renderer = {
          path = "crates/renderer";
          manifest = {
            package.name = "renderer";
            features = {
              default = [ ];
              native-mode = [ "render-lib/${nativeFeature}" ];
            };
            dependencies.render-lib = {
              version = "=1.0.0";
              features = [ "base" ];
              default-features = false;
            };
            build-dependencies.host-lib = {
              version = "=1.0.0";
              features = [ "host-only" ];
            };
          };
        };
        native-tool = {
          path = "crates/native-tool";
          manifest = {
            package.name = "native-tool";
            dependencies = {
              renderer = {
                path = "../renderer";
                features = [ "native-mode" ];
              };
              render-lib = {
                version = "=1.0.0";
                features = [ nativeFeature ];
              };
              native-only.version = "=1.0.0";
            };
          };
        };
      };
      targetMatches = target: _: target != "wasm";
      tables =
        manifest: mode:
        let
          select =
            table:
            (lib.optional (table ? dependencies) {
              kind = "normal";
              values = table.dependencies;
            })
            ++ (lib.optional (table ? build-dependencies) {
              kind = "build";
              values = table.build-dependencies;
            })
            ++ (lib.optional (mode.kind == "test" && table ? dev-dependencies) {
              kind = "dev";
              values = table.dev-dependencies;
            });
        in
        select manifest
        ++ lib.concatLists (
          lib.mapAttrsToList (
            predicate: table: lib.optionals (targetMatches mode.target predicate) (select table)
          ) (manifest.target or { })
        );
      dependencies =
        name: mode:
        lib.concatMap (
          table:
          lib.mapAttrsToList (alias: spec: {
            inherit alias spec;
            inherit (table) kind;
            name = if spec ? path then alias else null;
            path = if spec ? path then packages.${alias}.path else null;
          }) table.values
        ) (tables packages.${name}.manifest mode);
      mock = {
        inherit
          names
          packages
          dependencies
          targetMatches
          tables
          ;
        rootManifest = {
          workspace = {
            members = map (name: packages.${name}.path) names;
            package = {
              version = "0.1.0";
              edition = "2024";
            };
          };
          profile.dev = { };
        };
        workspaceDependency = _: value: value;
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
                    lib.filter (dependency: dependency.name != null) (
                      dependencies item.key (mode // { kind = if item.root then mode.kind else "build"; })
                    )
                  );
            }
          );
      };
    in
    import ../cargo-configurations.nix {
      inherit pkgs;
      sources = mock;
    };
  baseline = make "native-one";
  changed = make "native-two";
  wasmMode = {
    kind = "build";
    target = "wasm";
    featureRoots = [ "evm-abstract-web" ];
  };
  nativeMode = {
    kind = "build";
    target = "native";
  };
  wasm = baseline.resolverManifest wasmMode baseline.default;
  wasmChanged = changed.resolverManifest wasmMode changed.default;
  native = baseline.resolverManifest nativeMode baseline.default;
  nativeChanged = changed.resolverManifest nativeMode changed.default;
  dependencies = lib.concatMap (table: builtins.attrValues table.values) (
    # The helper's tables retain Cargo's host/target dependency distinction.
    (lib.optional (wasm ? dependencies) { values = wasm.dependencies; })
    ++ (lib.optional (wasm ? build-dependencies) { values = wasm.build-dependencies; })
    ++ lib.concatLists (
      lib.mapAttrsToList (_: table: [ { values = table.dependencies or { }; } ]) (wasm.target or { })
    )
  );
  packageNames = lib.sort builtins.lessThan (map (dependency: dependency.package) dependencies);
  features = lib.concatMap (dependency: dependency.features or [ ]) dependencies;
  renderLibrary = lib.findFirst (dependency: dependency.package == "render-lib") null dependencies;
in
assert builtins.toJSON wasm == builtins.toJSON wasmChanged;
assert builtins.toJSON native != builtins.toJSON nativeChanged;
assert
  packageNames == [
    "host-lib"
    "render-lib"
    "web-lib"
  ];
assert lib.all (dependency: !(dependency ? path)) dependencies;
assert !(renderLibrary.default-features or true);
assert lib.elem "browser" features && lib.elem "base" features && lib.elem "host-only" features;
assert !(lib.elem "native-one" features || lib.elem "native-two" features);
assert !(lib.hasInfix "native-mode" (baseline.packageArgs "renderer" wasmMode baseline.default));
assert !(wasm ? lib);
assert lib.all (target: (target.required-features or [ ]) != [ ]) wasm.bin;
true
