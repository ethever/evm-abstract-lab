{
  pkgs,
  sourceRoot,
  dependencies,
  ...
}:
let
  nixFiles = [
    (sourceRoot + "/flake.nix")
  ]
  ++ pkgs.lib.filter (path: pkgs.lib.hasSuffix ".nix" (toString path)) (
    pkgs.lib.filesystem.listFilesRecursive (sourceRoot + "/nix")
  );
in
{
  nix-format =
    pkgs.runCommand "evm-abstract-nix-format"
      {
        nativeBuildInputs = [ pkgs.nixfmt ];
      }
      ''
        nixfmt --check ${pkgs.lib.escapeShellArgs (map (path: "${path}") nixFiles)}
        touch $out
      '';
  toml-lint =
    pkgs.runCommand "evm-abstract-toml-lint"
      {
        nativeBuildInputs = [ pkgs.taplo ];
      }
      ''
        cd ${sourceRoot}
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
        cd ${sourceRoot}
        taplo fmt --check
        python ${sourceRoot}/scripts/test-toml-format.py
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
        inherit (dependencies.environment) SSL_CERT_FILE;
      }
      ''
        cd ${sourceRoot}
        lychee --config ./lychee.toml --offline --root-dir "$PWD" -- README.md '**/*.md'
        bash ${sourceRoot}/scripts/test-doc-links.sh ${sourceRoot}/lychee.toml
        touch $out
      '';
}
