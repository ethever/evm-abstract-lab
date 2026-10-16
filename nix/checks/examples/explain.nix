# Text explain paths are separate from shared JSON report validation.
{
  lib,
  inputs,
  concrete,
  mkCheck,
  ...
}:
let
  call = mkCheck "explain-call" [ ] { } ''
    evm-abstract explain --world ${inputs.file "examples/worlds/call-return-branch.json"} ${lib.escapeShellArgs concrete} > explain-call.txt
    grep -q 'Execution code' explain-call.txt
    grep -q 'Captured instruction list' explain-call.txt
    grep -q 'Verified cross-contract SSA:' explain-call.txt
    grep -q 'CALL result %' explain-call.txt
    grep -Eq '%[0-9]+ = CONST 0x[0-9a-f]+ ; origin=(PUSH[0-9]+|MLOAD|EQ)' explain-call.txt
    if grep -q 'opcode=' explain-call.txt; then exit 1; fi
    grep -Eq '^B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-call.txt
    if grep -Eq '^ +B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-call.txt; then exit 1; fi
    cp explain-call.txt "$out/"
  '';
  verbose = mkCheck "explain-verbose" [ ] { } ''
    evm-abstract explain --world ${inputs.file "examples/worlds/call-return-branch.json"} ${lib.escapeShellArgs concrete} --verbose > explain-verbose.txt
    grep -q 'captured frames:' explain-verbose.txt
    grep -q 'instruction effects:' explain-verbose.txt
    grep -q 'opcode=' explain-verbose.txt
    grep -Eq '^B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-verbose.txt
    if grep -Eq '^ +B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-verbose.txt; then exit 1; fi
    cp explain-verbose.txt "$out/"
  '';
  create = mkCheck "explain-create" [ ] { } ''
    evm-abstract explain --world ${inputs.file "examples/worlds/create-runtime.json"} ${lib.escapeShellArgs concrete} > explain-create.txt
    grep -q 'mode=InitCode' explain-create.txt
    grep -q 'mode=Runtime' explain-create.txt
    grep -q 'CREATE address %' explain-create.txt
    grep -Eq '^B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-create.txt
    if grep -Eq '^ +B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-create.txt; then exit 1; fi
    cp explain-create.txt "$out/"
  '';
  partial = mkCheck "explain-partial" [ ] { } ''
    evm-abstract explain --world ${inputs.file "examples/worlds/call-return-branch.json"} ${lib.escapeShellArgs concrete} --max-work 1 > explain-partial.txt && exit 1 || test "$?" = 2
    grep -q 'Input code observations (not execution evidence)' explain-partial.txt
    grep -q 'SSA unavailable' explain-partial.txt
    if grep -q 'Verified cross-contract SSA:' explain-partial.txt; then exit 1; fi
    grep -Eq '^B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-partial.txt
    if grep -Eq '^ +B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' explain-partial.txt; then exit 1; fi
    cp explain-partial.txt "$out/"
  '';
in
{
  checks = {
    example-explain-call = call;
    example-explain-verbose = verbose;
    example-explain-create = create;
    example-explain-partial = partial;
  };
  artifacts = [
    {
      name = "explain-call.txt";
      path = "${call}/explain-call.txt";
    }
    {
      name = "explain-verbose.txt";
      path = "${verbose}/explain-verbose.txt";
    }
    {
      name = "explain-create.txt";
      path = "${create}/explain-create.txt";
    }
    {
      name = "explain-partial.txt";
      path = "${partial}/explain-partial.txt";
    }
  ];
}
