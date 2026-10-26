# Keep the upstream version and lock checksum; only the input delta baseline is
# corrected. Native, Wasm and development Cargo consume this same vendor tree.
{ craneLib, src }:
craneLib.vendorCargoDeps {
  inherit src;
  overrideVendorCargoPackage =
    package: drv:
    if package.name == "egui" then
      assert package.version == "0.36.2";
      drv.overrideAttrs (old: {
        patches = (old.patches or [ ]) ++ [ ./patches/egui-0.36.2-pointer-press.patch ];
      })
    else
      drv;
}
