# Cargo children and compiler adapters must resolve the same patched egui as
# builds. Metadata uses dummy sources; the tiny Clippy probe compiles only egui.
{
  pkgs,
  craneLib,
  build,
  sourceRoot,
}:
let
  lintManifest = builtins.path {
    path = sourceRoot + "/lints/no_dyn/Cargo.toml";
    name = "no-dyn-Cargo.toml";
  };
  lintLock = builtins.path {
    path = sourceRoot + "/lints/no_dyn/Cargo.lock";
    name = "no-dyn-Cargo.lock";
  };
in
pkgs.runCommand "cargo-vendor-environment"
  {
    nativeBuildInputs = [
      pkgs.python3
      craneLib.cargo
      craneLib.clippy
      craneLib.rustc
    ];
  }
  ''
    export XDG_CACHE_HOME="$TMPDIR/cache"
    ${build.developmentCargoEnvironment}
    mkdir -p bin lint/src probe/src
    cat > bin/cargo-source-probe <<'SH'
    #!${pkgs.bash}/bin/bash
    shift
    exec "$CARGO" metadata "$@"
    SH
    chmod +x bin/cargo-source-probe
    export PATH="$PWD/bin:$PATH"
    cargo source-probe --manifest-path ${build.sources.dummy}/Cargo.toml \
      --locked --offline --format-version 1 > child-metadata.json
    cp ${lintManifest} lint/Cargo.toml
    cp ${lintLock} lint/Cargo.lock
    touch lint/src/lib.rs
    cargo metadata --manifest-path lint/Cargo.toml \
      --locked --offline --format-version 1 > lint-metadata.json
    cat > probe/Cargo.toml <<'TOML'
    [package]
    name = "vendor-child-probe"
    version = "0.0.0"
    edition = "2024"
    [dependencies]
    egui = { version = "=0.36.2", default-features = false }
    [workspace]
    TOML
    printf 'pub fn context() -> egui::Context { egui::Context::default() }\n' > probe/src/lib.rs
    cargo generate-lockfile --manifest-path probe/Cargo.toml --offline
    cargo clippy --manifest-path probe/Cargo.toml --locked --offline \
      --message-format=json -- -D warnings > clippy.jsonl
    mkdir -p "$out"
    python - ${build.common.cargoVendorDir}/config.toml "$out/result.json" <<'PYTHON'
    import json, pathlib, sys, tomllib
    config = tomllib.loads(pathlib.Path(sys.argv[1]).read_text())
    source = config['source'][config['source']['crates-io']['replace-with']]
    expected = (pathlib.Path(source['directory']) / 'egui-0.36.2/Cargo.toml').resolve()
    metadata = json.loads(pathlib.Path('child-metadata.json').read_text())
    egui, = [package for package in metadata['packages'] if package['name'] == 'egui']
    actual = pathlib.Path(egui['manifest_path']).resolve()
    assert egui['version'] == '0.36.2', egui
    assert actual == expected, (actual, expected)
    artifacts = [json.loads(line) for line in pathlib.Path('clippy.jsonl').read_text().splitlines()]
    egui_artifacts = [item for item in artifacts if item.get('reason') == 'compiler-artifact' and item['target']['name'] == 'egui']
    assert egui_artifacts, artifacts
    for item in egui_artifacts:
        assert pathlib.Path(item['target']['src_path']).resolve() == expected.parent / 'src/lib.rs', item
    lint = json.loads(pathlib.Path('lint-metadata.json').read_text())
    assert any(package['name'] == 'dylint_testing' for package in lint['packages'])
    pathlib.Path(sys.argv[2]).write_text(json.dumps(dict(egui_version=egui['version'], manifest=str(actual), clippy_source=str(expected.parent / 'src/lib.rs'), lint_packages=len(lint['packages']), vendor_config=sys.argv[1]), indent=2) + '\n')
    PYTHON
  ''
