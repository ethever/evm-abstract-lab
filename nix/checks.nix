# Each group owns checks for one boundary; common packages and inputs stay shared.
args:
(import ./checks/rust.nix args)
// (import ./checks/runtime.nix args)
// (import ./checks/tooling.nix args)
// (import ./checks/examples.nix args)
