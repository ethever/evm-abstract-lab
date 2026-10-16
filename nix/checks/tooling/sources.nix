# Formatting reads file contents; offline link validation also needs the names
# of non-document targets, but never their Rust/binary contents. Preserve those
# two dependency kinds separately so a code-only edit cannot rerun doc checks.
{ pkgs, sourceRoot }:
let
  lib = pkgs.lib;
  inputs = import ../../check-inputs.nix { inherit lib sourceRoot; };
  prefix = "${toString sourceRoot}/";
  relative = path: lib.removePrefix prefix (toString path);
  paths = map relative (lib.filesystem.listFilesRecursive sourceRoot);
  withSuffix = suffix: lib.filter (lib.hasSuffix suffix) paths;
  documentSuffixes = [
    ".md"
    ".html"
    ".htm"
    ".xhtml"
    ".svg"
  ];
  targetNames = builtins.toFile "documentation-targets.json" (builtins.toJSON paths);
  documents = lib.cleanSourceWith {
    src = sourceRoot;
    name = "documentation-inputs";
    filter =
      path: type:
      type == "directory"
      || type == "symlink"
      || lib.any (suffix: lib.hasSuffix suffix (lib.toLower path)) documentSuffixes;
  };
in
{
  nix = inputs.files (
    [ "flake.nix" ] ++ lib.filter (path: lib.hasPrefix "nix/" path) (withSuffix ".nix")
  );
  toml = inputs.files (withSuffix ".toml");
  # The LSP regression additionally opens Cargo.lock and executes its Python
  # harness. Plain Taplo lint never reads either input.
  tomlFormat = inputs.files (
    (withSuffix ".toml")
    ++ [
      "Cargo.lock"
      "scripts/test-toml-format.py"
    ]
  );
  linksConfig = inputs.file "lychee.toml";
  linksTest = inputs.file "scripts/test-doc-links.sh";
  linksProjectionTest = inputs.file "scripts/test-doc-inputs.py";
  # Fragment checks need real Markdown and HTML documents. Empty ordinary
  # targets retain file-existence checks, while actual symlinks preserve broken
  # link semantics. The projection regression exercises these exact outputs.
  docs =
    pkgs.runCommand "evm-abstract-documentation-inputs" { nativeBuildInputs = [ pkgs.python3 ]; }
      ''
        mkdir "$out"
        cp -a ${documents}/. "$out/"
        chmod -R u+w "$out"
        python - "$out" ${targetNames} <<'PY'
        import json
        from pathlib import Path
        import sys
        root = Path(sys.argv[1])
        for name in json.loads(Path(sys.argv[2]).read_text()):
            path = root / name
            if path.is_symlink():
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            if not path.exists():
                path.touch()
        PY
      '';
}
