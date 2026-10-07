# Dylint uses rustup's naming protocol even when Nix provides the compiler.
{
  pkgs,
  crane,
  lintRoot,
}:
let
  lib = pkgs.lib;
  compiler = pkgs.rust-bin.fromRustupToolchainFile (lintRoot + "/rust-toolchain.toml");
  channel =
    (builtins.fromTOML (builtins.readFile (lintRoot + "/rust-toolchain.toml"))).toolchain.channel;
  target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
  name = "${channel}-${target}";
  adapter = pkgs.symlinkJoin {
    name = "dylint-nix-toolchain";
    paths = [
      (pkgs.writeShellScriptBin "rustup" ''
        case "$*" in
          'show active-toolchain') printf '%s\n' '${name} (Nix)' ;;
          'which rustc') printf '%s\n' '${compiler}/bin/rustc' ;;
          'which cargo'|'+stable which cargo') printf '%s\n' '${compiler}/bin/cargo' ;;
          *) echo "Unsupported Nix rustup query: $*" >&2; exit 1 ;;
        esac
      '')
    ]
    ++
      map
        (
          command:
          pkgs.writeShellScriptBin command ''
            case "''${1-}" in
              '+${channel}'|'+${name}') shift ;;
              +*) echo "Nix Dylint requires ${channel}: $1" >&2; exit 1 ;;
            esac
            export RUSTUP_TOOLCHAIN='${name}'
            exec ${compiler}/bin/${command} "$@"
          ''
        )
        [
          "cargo"
          "rustc"
        ];
  };
  environment = driver: {
    DYLINT_DRIVER_PATH = "${driver}/lib/dylint";
    RUSTUP_HOME = "${driver}";
    RUSTUP_TOOLCHAIN = name;
    RUSTDOC = "${compiler}/bin/rustdoc";
    LD_LIBRARY_PATH = "${compiler}/lib";
    DYLD_LIBRARY_PATH = "${compiler}/lib";
  };
  enter = tools: ''
    export PATH="${
      lib.makeBinPath [
        adapter
        tools
        compiler
      ]
    }:$PATH"
  '';
in
{
  inherit
    compiler
    channel
    target
    name
    environment
    enter
    ;
  craneLib = (crane.mkLib pkgs).overrideToolchain compiler;
  activate =
    tools: driver:
    enter tools
    + lib.concatStringsSep "\n" (
      lib.mapAttrsToList (key: value: "export ${key}=${lib.escapeShellArg value}") (environment driver)
    );
}
