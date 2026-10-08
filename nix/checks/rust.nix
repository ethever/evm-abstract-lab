{ craneLib, build, ... }:
let
  forVariant =
    prefix: variant:
    let
      args = variant.common // {
        inherit (variant) cargoArtifacts;
      };
    in
    {
      "${prefix}tests" = craneLib.cargoTest (
        args // { cargoTestExtraArgs = "--all-targets -- --include-ignored"; }
      );
      # --all-targets excludes doctests; keep their execution explicit.
      "${prefix}doctests" = craneLib.cargoDocTest (
        args // { cargoTestExtraArgs = "-- --include-ignored"; }
      );
      "${prefix}clippy" = craneLib.cargoClippy (
        args // { cargoClippyExtraArgs = "--all-targets -- -D warnings"; }
      );
      "${prefix}docs" = craneLib.cargoDoc (args // { RUSTDOCFLAGS = "-D warnings"; });
    };
in
forVariant "" build.test
// forVariant "imbl-" build.imblTest
// forVariant "all-features-" build.allFeaturesTest
// {
  release-build = build.release.package;
  imbl-release-build = build.imblRelease.package;
  # cargo fmt uses --all rather than Cargo's --workspace/--locked flags.
  fmt = craneLib.cargoFmt (build.common // { cargoExtraArgs = "--all"; });
}
