{ craneLib, build, ... }:
let
  inherit (build)
    common
    cargoArtifacts
    package
    imblCommon
    imblArtifacts
    imblPackage
    ;
in
{
  build-and-test = package;
  imbl-build-and-test = imblPackage;
  clippy = craneLib.cargoClippy (
    common
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );
  imbl-clippy = craneLib.cargoClippy (
    imblCommon
    // {
      cargoArtifacts = imblArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    }
  );
  # cargo fmt uses --all rather than Cargo's --workspace/--locked flags.
  fmt = craneLib.cargoFmt (common // { cargoExtraArgs = "--all"; });
  docs = craneLib.cargoDoc (
    common
    // {
      inherit cargoArtifacts;
      RUSTDOCFLAGS = "-D warnings";
    }
  );
  imbl-docs = craneLib.cargoDoc (
    imblCommon
    // {
      cargoArtifacts = imblArtifacts;
      RUSTDOCFLAGS = "-D warnings";
    }
  );
}
