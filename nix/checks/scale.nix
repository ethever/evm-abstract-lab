# Profile/identity contracts use only scripts and frozen JSON, never the engine.
{ pkgs, sourceRoot, ... }:
let
  inputs = import ../check-inputs.nix {
    inherit (pkgs) lib;
    inherit sourceRoot;
  };
  sources = inputs.files [
    "scripts/benchmark-no-smt-scale.py"
    "scripts/test_benchmark_no_smt_scale.py"
    "scripts/generate-no-smt-scale-commands.py"
    "examples/no-smt-scale/code-only/dynamic-jump.json"
    "examples/no-smt-scale/code-only/loop.json"
    "examples/no-smt-scale/code-only/memory-copy.json"
    "examples/no-smt-scale/code-only/unknown-callback.json"
    "examples/no-smt-scale/code-only/unknown-input.json"
    "examples/no-smt-scale/commands-e325-native.json"
    "examples/no-smt-scale/manifest.json"
    "examples/no-smt-scale/wire14/usdc-original-request.json"
    "examples/no-smt-scale/wire14/weth-original-request.json"
    "examples/known-recursion/captured/manifest.json"
    "examples/known-recursion/captured/usdc-original-request.json"
    "examples/known-recursion/captured/weth-original-request.json"
    "examples/known-recursion/captured/usdc-26180958-observed-world.json"
    "examples/known-recursion/captured/weth-26180958-observed-world.json"
    "scripts/test_generate_no_smt_scale_commands.py"
    "scripts/benchmark_known_recursion/identity.py"
    "scripts/benchmark_known_recursion/process.py"
    "scripts/benchmark_known_recursion/report.py"
  ];
in
{
  no-smt-scale-contracts =
    pkgs.runCommand "evm-abstract-no-smt-scale-contracts" { nativeBuildInputs = [ pkgs.python3 ]; }
      ''
        export PYTHONDONTWRITEBYTECODE=1
        python -B ${sources}/scripts/test_benchmark_no_smt_scale.py
        python -B ${sources}/scripts/test_generate_no_smt_scale_commands.py
        touch "$out"
      '';
}
