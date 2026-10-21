# Preserve program text alignment and capacity/error contracts as small leaves.
{
  lib,
  inputs,
  concrete,
  mkCheck,
  ...
}:
let
  explain-program = mkCheck "explain-program" [ ] { } ''
    evm-abstract explain --file ${inputs.file "examples/straight-line.hex"} > explain-program.txt
    grep -q 'stack SSA:' explain-program.txt
    cp explain-program.txt "$out/"
  '';
  explain-aligned = mkCheck "explain-aligned" [ ] { } ''
    evm-abstract explain --file ${inputs.file "examples/known-bits-branch.hex"} --context-depth 0 > explain-aligned.txt
    grep -q '^B₀ @ 0x0000:$' explain-aligned.txt
    grep -q '^       0000: PUSH0' explain-aligned.txt
    cp explain-aligned.txt "$out/"
  '';
  ssa-aligned = mkCheck "ssa-aligned" [ ] { } ''
    evm-abstract ssa --file ${inputs.file "examples/dynamic-jump.hex"} > ssa-aligned.txt
    grep -q '^σᵖ₀ | context=\[\]:$' ssa-aligned.txt
    grep -q '^B₀ @ 0x0000:$' ssa-aligned.txt
    grep -q '^       0000: %0 = CONST 0x0 ; origin=PUSH1$' ssa-aligned.txt
    grep -q '^       0002: %1 = CALLDATALOAD 0x0$' ssa-aligned.txt
    cp ssa-aligned.txt "$out/"
  '';
  ssa-world-aligned = mkCheck "ssa-world-aligned" [ ] { } ''
    evm-abstract analyze --world ${inputs.file "examples/worlds/call-return-branch.json"} ${lib.escapeShellArgs concrete} --ssa > ssa-world-aligned.txt
    grep -q '^       0000: %0 = CONST 0x20 ; origin=PUSH1 | opcode=0x60' ssa-world-aligned.txt
    if grep -Eq '^ +pc=0x[0-9a-f]+:' ssa-world-aligned.txt; then exit 1; fi
    cp ssa-world-aligned.txt "$out/"
  '';
  explain-capacity = mkCheck "explain-capacity" [ ] { } ''
    evm-abstract explain --hex 5f351e00 --numeric-model finite-set --max-constants 257 > explain-capacity.txt
    grep -q 'status=Converged' explain-capacity.txt
    evm-abstract cfg --hex 5f351e00 --numeric-model finite-set --max-constants 257 --format json |
      jq -e '.domain_spec.capacity == 257 and (.states[0].exit_stack[0].numeric.constants | length == 257)' > /dev/null
    cp explain-capacity.txt "$out/"
  '';
  explain-capacity-work = mkCheck "explain-capacity-work" [ ] { } ''
    evm-abstract explain --hex 5f351e00 --numeric-model finite-set --max-constants 1000 --max-work 20000000 > explain-capacity-work.txt && exit 1 || test "$?" = 2
    grep -q 'status=Incomplete' explain-capacity-work.txt
    grep -q 'Work' explain-capacity-work.txt
    grep -q 'SSA unavailable' explain-capacity-work.txt
    cp explain-capacity-work.txt "$out/"
  '';
in
{
  checks = {
    example-explain-program = explain-program;
    example-explain-aligned = explain-aligned;
    example-ssa-aligned = ssa-aligned;
    example-ssa-world-aligned = ssa-world-aligned;
    example-explain-capacity = explain-capacity;
    example-explain-capacity-work = explain-capacity-work;
  };
  artifacts = [
    {
      name = "explain-program.txt";
      path = "${explain-program}/explain-program.txt";
    }
    {
      name = "explain-aligned.txt";
      path = "${explain-aligned}/explain-aligned.txt";
    }
    {
      name = "ssa-aligned.txt";
      path = "${ssa-aligned}/ssa-aligned.txt";
    }
    {
      name = "ssa-world-aligned.txt";
      path = "${ssa-world-aligned}/ssa-world-aligned.txt";
    }
    {
      name = "explain-capacity.txt";
      path = "${explain-capacity}/explain-capacity.txt";
    }
    {
      name = "explain-capacity-work.txt";
      path = "${explain-capacity-work}/explain-capacity-work.txt";
    }
  ];
}
