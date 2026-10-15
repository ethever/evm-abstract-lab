# Evaluation-only regressions: extending a Cargo feature/profile matrix must
# change cache identity without depending on a hand-maintained feature list.
{ pkgs, sources }:
let
  make =
    {
      extra ? false,
      weak ? false,
      testOpt ? 0,
      panic ? "unwind",
    }:
    let
      engine = {
        path = "../engine";
        optional = weak;
      };
      fast = {
        version = "=1.0.0";
        optional = true;
      };
      mock = {
        names = [
          "app"
          "engine"
        ];
        rootManifest.profile = {
          dev = {
            opt-level = 0;
            inherit panic;
          };
          test.opt-level = testOpt;
        };
        packages = {
          app.manifest = {
            package.name = "app";
            features = {
              default = pkgs.lib.optional weak "engine?/imbl";
              imbl = [ (if weak then "dep:engine" else "engine/imbl") ];
            }
            // pkgs.lib.optionalAttrs extra { additional = [ "engine/additional" ]; };
            dependencies = { inherit engine; };
          };
          engine.manifest = {
            package.name = "engine";
            features = {
              default = [ ];
              imbl = [ "dep:fast" ];
            }
            // pkgs.lib.optionalAttrs extra { additional = [ ]; };
            dependencies = { inherit fast; };
          };
        };
        dependencies = name: _: [
          {
            kind = "normal";
            alias = if name == "app" then "engine" else "fast";
            name = if name == "app" then "engine" else null;
            spec = if name == "app" then engine else fast;
          }
        ];
        workspaceDependency = _: value: value;
        targetMatches = _: _: true;
      };
    in
    import ../cargo-configurations.nix {
      inherit pkgs;
      sources = mock;
    };
  baseline = make { };
  extended = make { extra = true; };
  weak = make { weak = true; };
  changedProfile = make { testOpt = 1; };
  abortProfile = make { panic = "abort"; };
  inheritedSerde = sources.workspaceDependency "serde" {
    workspace = true;
    features = [ "rc" ];
  };
in
assert baseline.default.signature != baseline.imbl.signature;
assert baseline.imbl.signature == baseline.allFeatures.signature;
assert extended.imbl.signature != extended.allFeatures.signature;
assert !(pkgs.lib.elem "imbl" weak.default.packages.engine.features);
assert pkgs.lib.elem "imbl" weak.imbl.packages.engine.features;
assert baseline.libraryProfileFor "test" == "dev";
assert changedProfile.libraryProfileFor "test" == "test";
assert abortProfile.libraryProfileFor "test" == "test";
assert pkgs.lib.elem "derive" inheritedSerde.features && pkgs.lib.elem "rc" inheritedSerde.features;
true
