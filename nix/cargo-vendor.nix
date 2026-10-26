# Keep the upstream version and lock checksum; only the input delta baseline is
# corrected. Native, Wasm and development Cargo consume this same vendor tree.
{
  craneLib,
  src,
  extraLocks ? [ ],
}:
let
  lock = builtins.fromTOML (builtins.readFile (src + "/Cargo.lock"));
  expected =
    package:
    package.name == "egui"
    && package.version == "0.36.2"
    && (package.source or "") == "registry+https://github.com/rust-lang/crates.io-index"
    && (package.checksum or "") == "dc938cc27cd911415e1e4d151bc56ff9df063d8db081374e87aeb805ea74b2ba";
  overrideVendorCargoPackage =
    package: drv:
    if package.name == "egui" then
      assert expected package;
      drv.overrideAttrs (old: {
        patches = (old.patches or [ ]) ++ [ ./patches/egui-0.36.2-pointer-press.patch ];
      })
    else
      drv;
in
assert builtins.any expected lock.package;
if extraLocks == [ ] then
  craneLib.vendorCargoDeps { inherit src overrideVendorCargoPackage; }
else
  # The compiler plugin has a separate lock. Only development needs their union;
  # production caches continue to depend on the application lock alone.
  craneLib.vendorMultipleCargoDeps {
    inherit overrideVendorCargoPackage;
    cargoConfigs = (craneLib.findCargoFiles src).cargoConfigs;
    cargoLockParsedList = [ lock ] ++ map (file: builtins.fromTOML (builtins.readFile file)) extraLocks;
  }
