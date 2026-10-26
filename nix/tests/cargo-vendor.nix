# Resolve real Cargo metadata through the development command. Dummy workspace
# sources keep this parity check independent of application implementation edits.
{
  pkgs,
  craneLib,
  build,
}:
pkgs.runCommand "cargo-vendor-environment"
  {
    nativeBuildInputs = [
      pkgs.python3
      craneLib.rustc
    ];
  }
  ''
    export CARGO_HOME="$TMPDIR/cargo-home"
    ${build.cargoCommand}/bin/cargo metadata \
      --manifest-path ${build.sources.dummy}/Cargo.toml \
      --locked --offline --format-version 1 > metadata.json
    mkdir -p "$out"
    python - ${build.common.cargoVendorDir}/config.toml "$out/result.json" <<'PY'
    import json, pathlib, sys, tomllib
    config = tomllib.loads(pathlib.Path(sys.argv[1]).read_text())
    source = config['source'][config['source']['crates-io']['replace-with']]
    expected = (pathlib.Path(source['directory']) / 'egui-0.36.2/Cargo.toml').resolve()
    metadata = json.loads(pathlib.Path('metadata.json').read_text())
    egui, = [package for package in metadata['packages'] if package['name'] == 'egui']
    actual = pathlib.Path(egui['manifest_path']).resolve()
    assert egui['version'] == '0.36.2', egui
    assert actual == expected, (actual, expected)
    pathlib.Path(sys.argv[2]).write_text(json.dumps(dict(egui_version=egui['version'], manifest=str(actual), vendor_config=sys.argv[1]), indent=2) + '\n')
    PY
  ''
