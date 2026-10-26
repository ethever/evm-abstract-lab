# Development dependencies are explicit and independent of the check graph.
{
  craneLib,
  dependencies,
  dylint,
  web,
  cargoCommand,
}:
craneLib.devShell (
  dependencies.environment
  // {
    buildInputs = dependencies.buildInputs ++ dependencies.developmentLibraries;
    # Crane intentionally filters nativeBuildInputs from devShell arguments;
    # mkShell installs packages as native inputs, including setup hooks.
    packages =
      dependencies.nativeBuildInputs
      ++ dependencies.developmentTools
      ++ dylint.devPackages
      ++ web.devPackages
      ++ [ cargoCommand ];
    # Crane prepends its toolchain Cargo. Prefer the shared vendor-aware command
    # for terminal and rust-analyzer subprocesses without changing user files.
    shellHook = ''
      export PATH="${cargoCommand}/bin:$PATH"
    '';
    RUST_BACKTRACE = "1";
    FONTCONFIG_FILE = dependencies.fontConfig;
  }
)
