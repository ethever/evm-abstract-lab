{ pkgs, toolchain }:
let
  inherit (pkgs) lib;
  supported = builtins.elem pkgs.stdenv.hostPlatform.system [
    "x86_64-linux"
    "aarch64-linux"
    "aarch64-darwin"
  ];
  extensions = import ./editor/extensions.nix { inherit pkgs toolchain; };
  # The pinned Linux CLI script retains /usr/bin/env, unavailable in a Nix
  # sandbox. Fix its interpreter while preserving the signed Darwin bundle.
  code = pkgs.vscode.overrideAttrs (old: {
    postFixup =
      (old.postFixup or "")
      + lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
        substituteInPlace "$out/bin/.code-wrapped" \
          --replace-fail '#!/usr/bin/env sh' '#!${pkgs.runtimeShell}'
      '';
  });
  vscode = pkgs.vscode-with-extensions.override {
    vscode = code;
    vscodeExtensions = extensions.all;
  };
  launcher = pkgs.writeShellApplication {
    name = "evm-abstract-vscode";
    runtimeInputs = [ pkgs.nix ];
    # Resolve the current checkout's devShell instead of copying its variables.
    # A separate profile prevents an existing unmanaged Code process taking over.
    text = ''
      unset VSCODE_IPC_HOOK_CLI
      if [ "$#" -eq 0 ]; then set -- "$PWD"; fi
      exec nix develop --no-update-lock-file --command ${vscode}/bin/code \
        --user-data-dir "''${XDG_STATE_HOME:-$HOME/.local/state}/evm-abstract/vscode" "$@"
    '';
  };
  expectedExtensions = pkgs.writeText "evm-abstract-vscode-extensions.json" (
    builtins.toJSON (
      map (extension: {
        id = extension.vscodeExtUniqueId;
        inherit (extension) version;
      }) extensions.all
    )
  );
in
{
  # The pinned Microsoft archive does not support x86_64-darwin. Keep the
  # existing editor + repository bootstrap available there without a GUI output.
  packages = {
    vscode-extensions = extensions.archives;
    install-vscode-extensions = extensions.installer;
  }
  // lib.optionalAttrs supported {
    inherit vscode;
    vscode-launcher = launcher;
  };
  apps = {
    install-vscode-extensions = {
      type = "app";
      program = lib.getExe extensions.installer;
      meta.description = "Install this flake's pinned Rust/TOML extensions into the existing VS Code session";
    };
  }
  // lib.optionalAttrs supported {
    vscode = {
      type = "app";
      program = lib.getExe launcher;
      meta.description = "Open the pinned VS Code and extensions in this checkout's Nix environment";
    };
  };
  checks = lib.optionalAttrs supported {
    vscode = pkgs.runCommand "evm-abstract-vscode-check" { nativeBuildInputs = [ pkgs.jq ]; } ''
      set -euo pipefail
      ${vscode}/bin/code --user-data-dir "$TMPDIR/editor-version" --version > version
      test "$(head -n 1 version)" = ${lib.escapeShellArg pkgs.vscode.version}
      ${vscode}/bin/code --user-data-dir "$TMPDIR/editor-extensions" \
        --list-extensions --show-versions > extensions
      jq -r '.[] | .id + "@" + .version' ${expectedExtensions} | sort > expected
      sort extensions > actual
      diff -u expected actual
      ${lib.concatMapStringsSep "\n" (extension: ''
        ${code}/bin/code --user-data-dir "$TMPDIR/editor-vsix-user" \
          --extensions-dir "$TMPDIR/editor-vsix-extensions" \
          --install-extension ${extensions.archives}/${extensions.archiveName extension} --force
      '') extensions.all}
      ${code}/bin/code --user-data-dir "$TMPDIR/editor-vsix-user" \
        --extensions-dir "$TMPDIR/editor-vsix-extensions" \
        --list-extensions --show-versions | sort > installed
      diff -u expected installed
      jq '[.[] | {id: .identifier.id, pinned: .metadata.pinned, source: .metadata.source}]' \
        "$TMPDIR/editor-vsix-extensions/extensions.json" > extension-policy.json
      jq -e 'length == 2 and all(.[]; .pinned == true and .source == "vsix")' extension-policy.json
      jq -e --arg server ${lib.escapeShellArg (lib.getExe pkgs.taplo)} '
        .contributes.configuration.properties."evenBetterToml.taplo.bundled".default == false and
        .contributes.configuration.properties."evenBetterToml.taplo.path".default == $server
      ' "$TMPDIR/editor-vsix-extensions/tamasfe.even-better-toml-${extensions.toml.version}/package.json"
      jq -e --arg server ${lib.escapeShellArg "${toolchain}/bin/rust-analyzer"} '
        any(.contributes.configuration[]; .properties."rust-analyzer.server.path".default? == $server)
      ' "$TMPDIR/editor-vsix-extensions/rust-lang.rust-analyzer-${extensions.rust.version}/package.json"
      mkdir "$out"
      cp version extensions installed extension-policy.json "$out/"
    '';
  };
}
