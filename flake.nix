{
  description = "EVM abstract interpretation learning lab: Rust, CFG and stack SSA";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      eachSystem = f: nixpkgs.lib.genAttrs systems (system: f system);
      perSystem = eachSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          # One version source for both rustup and Nix; no floating `stable.latest`.
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          fontConfig = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              craneLib.filterCargoSources path type
              || pkgs.lib.hasSuffix ".hex" path
              || pkgs.lib.hasSuffix ".json" path;
          };
          common = {
            inherit src;
            strictDeps = true;
            pname = "evm-abstract";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
            cargoExtraArgs = "--workspace --locked";
            meta = {
              description = "Learn EVM abstract interpretation, contextual CFGs and stack SSA";
              license = pkgs.lib.licenses.mit;
              mainProgram = "evm-abstract";
            };
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          package = craneLib.buildPackage (common // { inherit cargoArtifacts; });
        in
        {
          packages.default = package;
          apps.default = {
            type = "app";
            program = "${package}/bin/evm-abstract";
            meta = package.meta;
          };
          checks = {
            build-and-test = package;
            clippy = craneLib.cargoClippy (
              common
              // {
                inherit cargoArtifacts;
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
            nix-format =
              pkgs.runCommand "evm-abstract-nix-format"
                {
                  nativeBuildInputs = [ pkgs.nixfmt ];
                }
                ''
                  nixfmt --check ${./flake.nix}
                  touch $out
                '';
            doc-links =
              pkgs.runCommand "evm-abstract-doc-links"
                {
                  nativeBuildInputs = [ pkgs.python3 ];
                }
                ''
                  python ${./scripts/check-doc-links.py} ${self}
                  touch $out
                '';
            examples =
              pkgs.runCommand "evm-abstract-installed-examples"
                {
                  nativeBuildInputs = [
                    package
                    pkgs.graphviz
                    pkgs.jq
                  ];
                  FONTCONFIG_FILE = fontConfig;
                }
                ''
                  export XDG_CACHE_HOME="$TMPDIR"
                  for fixture in ${./examples}/*.hex; do
                    evm-abstract cfg --file "$fixture" --format json | jq -e '.status == "Converged"' > /dev/null
                    evm-abstract ssa --file "$fixture" --format json | jq -e '.ssa.value_count > 0' > /dev/null
                  done
                  evm-abstract cfg --file ${./examples/internal-calls.hex} --context-depth 1 --format json |
                    jq -e '[.states[] | select(.key.context != [])] | length > 0' > /dev/null
                  evm-abstract cfg --file ${./examples/diamond.hex} --format dot > diamond.dot
                  dot -Tsvg diamond.dot -o diamond.svg
                  test -s diamond.svg
                  mkdir $out
                  cp diamond.dot diamond.svg $out/
                '';
          };
          devShells.default = craneLib.devShell {
            checks = self.checks.${system};
            packages = with pkgs; [
              graphviz
              cargo-nextest
              nixfmt
              git
              nixVersions.stable
            ];
            RUST_BACKTRACE = "1";
            FONTCONFIG_FILE = fontConfig;
          };
          formatter = pkgs.nixfmt;
        }
      );
    in
    {
      packages = eachSystem (system: perSystem.${system}.packages);
      apps = eachSystem (system: perSystem.${system}.apps);
      checks = eachSystem (system: perSystem.${system}.checks);
      devShells = eachSystem (system: perSystem.${system}.devShells);
      formatter = eachSystem (system: perSystem.${system}.formatter);
    };
}
