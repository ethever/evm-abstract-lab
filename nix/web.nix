# Public Web interface. Rust/Wasm artifacts and browser acceptance scheduling
# have separate source boundaries; this entrypoint only assembles their outputs.
args:
let
  assets = import ./web/assets.nix args;
  browser = import ./web/browser.nix (args // { inherit (assets) assets; });
in
assets // { checks = assets.checks // browser; }
