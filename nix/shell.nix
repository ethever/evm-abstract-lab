# Development dependencies are explicit and independent of the check graph.
{
  craneLib,
  dependencies,
  dylint,
}:
craneLib.devShell (
  dependencies.environment
  // {
    buildInputs = dependencies.buildInputs ++ dependencies.developmentLibraries;
    # Crane intentionally filters nativeBuildInputs from devShell arguments;
    # mkShell installs packages as native inputs, including setup hooks.
    packages = dependencies.nativeBuildInputs ++ dependencies.developmentTools ++ dylint.devPackages;
    RUST_BACKTRACE = "1";
    FONTCONFIG_FILE = dependencies.fontConfig;
  }
)
