{ pkgs, toolchain }:
let
  inherit (pkgs) lib;
  rust = pkgs.vscode-extensions.rust-lang.rust-analyzer.override {
    rust-analyzer = toolchain;
  };
  toml = pkgs.vscode-extensions.tamasfe.even-better-toml.overrideAttrs (old: {
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.jq ];
    preInstall = (old.preInstall or "") + ''
      jq --arg server ${lib.escapeShellArg (lib.getExe pkgs.taplo)} '
        .contributes.configuration.properties."evenBetterToml.taplo.bundled".default = false |
        .contributes.configuration.properties."evenBetterToml.taplo.path".default = $server
      ' package.json > package.json.new
      mv package.json.new package.json
    '';
  });
  all = [
    rust
    toml
  ];
  archiveName = extension: "${extension.vscodeExtUniqueId}-${extension.version}.vsix";
  archives =
    pkgs.runCommand "evm-abstract-vscode-extensions"
      {
        nativeBuildInputs = [
          pkgs.zip
          pkgs.unzip
        ];
      }
      ''
        mkdir -p "$out/nix-support"
        # VSIX files are compressed, so retain native server references separately.
        printf '%s\n' ${toolchain} ${pkgs.taplo} > "$out/nix-support/runtime-closure"
        ${lib.concatMapStringsSep "\n" (extension: ''
          mkdir ${extension.vscodeExtUniqueId}
          cd ${extension.vscodeExtUniqueId}
          unzip -q ${extension.src}
          test -f '[Content_Types].xml'
          test -f extension.vsixmanifest
          rm -rf extension
          cp -rL ${extension}/share/vscode/extensions/${extension.vscodeExtUniqueId} extension
          # VS Code appends installation metadata to the extracted package.json.
          chmod -R u+w extension
          find . -type f -print | LC_ALL=C sort | zip -X -q "$out/${archiveName extension}" -@
          cd ..
        '') all}
      '';
  installer = pkgs.writeShellApplication {
    name = "install-vscode-extensions";
    runtimeInputs = [
      pkgs.nix
      pkgs.coreutils
      pkgs.gnugrep
    ];
    # The existing CLI determines Local versus Remote SSH installation scope.
    # Invoke explicitly from that editor's integrated terminal; never at startup.
    text = ''
      command -v code >/dev/null || {
        echo "Run this app inside the existing VS Code integrated terminal (code must be on PATH)." >&2
        exit 1
      }
      editor_state="''${XDG_STATE_HOME:-$HOME/.local/state}/evm-abstract/vscode-extensions"
      mkdir -p "$editor_state"
      # Keep old roots too: a failed second install must not orphan the first
      # extension's previous server. Each bundle has its own immutable root.
      nix-store --add-root "$editor_state/${builtins.baseNameOf (toString archives)}" \
        --indirect --realise ${archives} >/dev/null
      ${lib.concatMapStringsSep "\n" (extension: ''
        code --install-extension ${archives}/${archiveName extension} --force
      '') all}
      installed_extensions=$(code --list-extensions --show-versions)
      ${lib.concatMapStringsSep "\n" (extension: ''
        printf '%s\n' "$installed_extensions" | grep -Fx ${lib.escapeShellArg "${extension.vscodeExtUniqueId}@${extension.version}"}
      '') all}
    '';
  };
in
{
  inherit
    all
    archives
    installer
    archiveName
    rust
    toml
    ;
}
