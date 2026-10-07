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
    inputs:
    let
      # The locked nixpkgs supports these platforms; x86_64-darwin was removed.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      eachSystem = f: inputs.nixpkgs.lib.genAttrs systems f;
      perSystem = eachSystem (system: import ./nix/system.nix { inherit inputs system; });
    in
    {
      packages = eachSystem (system: perSystem.${system}.packages);
      apps = eachSystem (system: perSystem.${system}.apps);
      checks = eachSystem (system: perSystem.${system}.checks);
      devShells = eachSystem (system: perSystem.${system}.devShells);
      formatter = eachSystem (system: perSystem.${system}.formatter);
    };
}
