{
  description = "Cross-contract EVM abstract analysis learning lab: worlds, calls and SSA";

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
            # AWS-LC selects its CMake backend for supported fallback/ASM configurations.
            nativeBuildInputs = [ pkgs.cmake ];
            # The RPC client initializes platform TLS roots even for HTTP fixtures.
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            meta = {
              description = "Analyze multi-account EVM worlds, cross-contract graphs and SSA";
              license = pkgs.lib.licenses.mit;
              mainProgram = "evm-abstract";
            };
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          package = craneLib.buildPackage (common // { inherit cargoArtifacts; });
          imblCommon = common // {
            cargoExtraArgs = "--workspace --locked --features imbl";
          };
          imblArtifacts = craneLib.buildDepsOnly imblCommon;
          imblPackage = craneLib.buildPackage (imblCommon // { cargoArtifacts = imblArtifacts; });
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
            state-backend-parity =
              pkgs.runCommand "evm-abstract-state-backend-parity" { nativeBuildInputs = [ pkgs.jq ]; }
                ''
                  set -euo pipefail
                  mkdir std imbl
                  for world in ${./examples/worlds}/*.json; do
                    name="$(basename "$world" .json)"
                    std_status=0
                    imbl_status=0
                    ${package}/bin/evm-abstract analyze --world "$world" \
                      --entry 0x0000000000000000000000000000000000000101 \
                      --format json > "std/$name.json" || std_status=$?
                    ${imblPackage}/bin/evm-abstract analyze --world "$world" \
                      --entry 0x0000000000000000000000000000000000000101 \
                      --format json > "imbl/$name.json" || imbl_status=$?
                    test "$std_status" -eq "$imbl_status"
                    case "$std_status" in 0|2) ;; *) exit 1 ;; esac
                    cmp "std/$name.json" "imbl/$name.json"
                    if test "$std_status" -eq 0; then
                      ${package}/bin/evm-abstract analyze --world "$world" \
                        --entry 0x0000000000000000000000000000000000000101 \
                        --format json --ssa > "std/$name-ssa.json"
                      ${imblPackage}/bin/evm-abstract analyze --world "$world" \
                        --entry 0x0000000000000000000000000000000000000101 \
                        --format json --ssa > "imbl/$name-ssa.json"
                      cmp "std/$name-ssa.json" "imbl/$name-ssa.json"
                    else
                      jq -e '.status == "Incomplete"' "std/$name.json" > /dev/null
                    fi
                  done
                  mkdir $out
                  cp -r std imbl $out/
                '';
            nix-format =
              pkgs.runCommand "evm-abstract-nix-format"
                {
                  nativeBuildInputs = [ pkgs.nixfmt ];
                }
                ''
                  nixfmt --check ${./flake.nix}
                  touch $out
                '';
            toml-lint =
              pkgs.runCommand "evm-abstract-toml-lint"
                {
                  nativeBuildInputs = [ pkgs.taplo ];
                }
                ''
                  cd ${self}
                  taplo lint --no-schema
                  touch $out
                '';
            toml-format =
              pkgs.runCommand "evm-abstract-toml-format"
                {
                  nativeBuildInputs = [
                    pkgs.taplo
                    pkgs.python3
                  ];
                }
                ''
                  cd ${self}
                  taplo fmt --check
                  python ${./scripts/test-toml-format.py}
                  touch $out
                '';
            doc-links =
              pkgs.runCommand "evm-abstract-doc-links"
                {
                  nativeBuildInputs = [
                    pkgs.lychee
                    pkgs.bash
                  ];
                  # Lychee initializes its TLS client even with --offline.
                  SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
                }
                ''
                  cd ${self}
                  lychee --config ./lychee.toml --offline --root-dir "$PWD" -- README.md '**/*.md'
                  bash ${./scripts/test-doc-links.sh} ${./lychee.toml}
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
                  set -euo pipefail
                  export XDG_CACHE_HOME="$TMPDIR"
                  for world in ${./examples/worlds}/*.json; do
                    case "$world" in
                      */missing-code.json) continue ;;
                    esac
                    evm-abstract analyze --world "$world" --entry 0x0000000000000000000000000000000000000101 --format json |
                      jq -e '.status == "Converged" and .world.fork == "osaka" and any(.edges[]; .kind == "Call")' > /dev/null
                    evm-abstract analyze --world "$world" --entry 0x0000000000000000000000000000000000000101 --format json --ssa |
                      jq -e '.analysis.status == "Converged" and (.ssa | type == "object")' > /dev/null
                  done
                  if evm-abstract analyze --world ${./examples/worlds/missing-code.json} --entry 0x0000000000000000000000000000000000000101 --format json > missing.json; then
                    exit 1
                  else
                    test "$?" -eq 2
                  fi
                  jq -e '.status == "Incomplete" and any(.frontiers[]; .reason | has("MissingCode"))' missing.json > /dev/null
                  evm-abstract analyze --world ${./examples/worlds/summary-reuse.json} --entry 0x0000000000000000000000000000000000000101 --format json > summary-on.json
                  evm-abstract analyze --world ${./examples/worlds/summary-reuse.json} --entry 0x0000000000000000000000000000000000000101 --format json --no-summaries > summary-off.json
                  jq -e '.status == "Converged" and .summary_stats.hits > 0 and .summary_stats.imported_states > 0 and any(.summaries[]; (.reused_at | length) > 0) and .world.identity.kind == "offline" and (.world.fingerprint | test("^0x[0-9a-f]{64}$"))' summary-on.json > /dev/null
                  jq -e '.status == "Converged" and .summary_stats.hits == 0 and .summaries == []' summary-off.json > /dev/null
                  jq -S '[.outcomes[] | {kind,data,store}] | unique' summary-on.json > on-relations.json
                  jq -S '[.outcomes[] | {kind,data,store}] | unique' summary-off.json > off-relations.json
                  cmp on-relations.json off-relations.json
                  evm-abstract analyze --world ${./examples/worlds/create-runtime.json} --entry 0x0000000000000000000000000000000000000101 --format json > creation.json
                  jq -e 'any(.states[]; .key.frames[-1].mode == "InitCode") and any(.states[]; .key.frames[-1].mode == "Runtime" and .key.frames[-1].code_hash == "0x30962a84ef989ca0f724a5b2ec94f9cbf6a731752ce2c0be5333bf96e460c9fd") and any(.outcomes[]; .store.nonces["0x0000000000000000000000000000000000000101"].Constants == ["0x1"] and any(.store.account_observations[]; .address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7" and .existence == "present" and .code_size == 8 and .nonce.Constants == ["0x1"]))' creation.json > /dev/null
                  evm-abstract analyze --world ${./examples/worlds/created-selfdestruct.json} --entry 0x0000000000000000000000000000000000000101 --format json > destruction.json
                  jq -e 'any(.states[]; .entry.store.pending_destruction["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"] == true and .entry.store.codes["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"].Runtime.byte_len == 8) and any(.outcomes[]; .kind == "Return" and .store.balances["0x0000000000000000000000000000000000000200"].Constants == ["0x7"] and any(.store.account_observations[]; .address == "0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb" and .existence == "absent" and .code_size == 0 and .nonce.Constants == ["0x0"]))' destruction.json > /dev/null
                  evm-abstract analyze --world ${./examples/worlds/identity-precompile.json} --entry 0x0000000000000000000000000000000000000101 --format json > native.json
                  jq -e 'any(.states[]; .key.frames[-1].mode == {"Precompile":"0x0000000000000000000000000000000000000004"}) and any(.outcomes[]; .kind == "Return" and ((.data.bytes["31"].Constants // []) | index("0x2a")) != null)' native.json > /dev/null
                  evm-abstract analyze --world ${./examples/worlds/summary-reuse.json} --entry 0x0000000000000000000000000000000000000101 > summary.txt
                  grep -Eq 'hits=[1-9][0-9]*' summary.txt
                  grep -q 'reused_at=' summary.txt
                  grep -q 'fingerprint=0x' summary.txt
                  evm-abstract analyze --world ${./examples/worlds/summary-reuse.json} --entry 0x0000000000000000000000000000000000000101 --format dot > summaries.dot
                  dot -Tsvg summaries.dot -o summaries.svg
                  test -s summaries.svg
                  grep -q 'label="reused"' summaries.dot
                  evm-abstract analyze --world ${./examples/worlds/create-runtime.json} --entry 0x0000000000000000000000000000000000000101 --format dot > creation.dot
                  dot -Tsvg creation.dot -o creation.svg
                  test -s creation.svg
                  evm-abstract analyze --world ${./examples/worlds/proxy-storage.json} --entry 0x0000000000000000000000000000000000000101 --format dot > proxies.dot
                  dot -Tsvg proxies.dot -o proxies.svg
                  test -s proxies.svg
                  for fixture in ${./examples}/*.hex; do
                    evm-abstract cfg --file "$fixture" --format json | jq -e '.status == "Converged" and .program.fork == "osaka"' > /dev/null
                    evm-abstract ssa --file "$fixture" --format json | jq -e '.ssa.value_count > 0' > /dev/null
                  done
                  evm-abstract cfg --file ${./examples/osaka-clz.hex} --format json |
                    jq -e '.edges | length == 1' > /dev/null
                  for fork in cancun prague; do
                    evm-abstract cfg --file ${./examples/osaka-clz.hex} --fork "$fork" --format json |
                      jq -e '.edges == [] and any(.diagnostics[]; .kind == "InvalidOpcode" and .pc == 2)' > /dev/null
                  done
                  evm-abstract cfg --file ${./examples/internal-calls.hex} --context-depth 1 --format json |
                    jq -e '[.states[] | select(.key.context != [])] | length > 0' > /dev/null
                  evm-abstract cfg --file ${./examples/diamond.hex} --format dot > diamond.dot
                  dot -Tsvg diamond.dot -o diamond.svg
                  test -s diamond.svg
                  mkdir $out
                  cp diamond.dot diamond.svg proxies.dot proxies.svg summaries.dot summaries.svg creation.dot creation.svg summary-on.json summary-off.json creation.json destruction.json native.json summary.txt $out/
                '';
          };
          devShells.default = craneLib.devShell {
            checks = self.checks.${system};
            packages = with pkgs; [
              graphviz
              cargo-nextest
              nixfmt
              taplo
              lychee
              git
              python3
              coreutils
              bash
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
