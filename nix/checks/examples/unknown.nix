# Every lesson is its own numeric contract. The validator reads the existing
# analysis envelope and fails closed for any newly introduced lesson name.
{
  pkgs,
  lib,
  fixtures,
  inputs,
  mkCheck,
  ...
}:
let
  validator = inputs.file "scripts/check-example-unknown-world.py";
  environment = [
    "--evm.to"
    "0x0000000000000000000000000000000000000101"
    "--evm.caller"
    "0x0000000000000000000000000000000000000900"
    "--evm.value"
    "0"
    "--evm.calldata"
    "0x"
  ];
in
{
  checks = lib.listToAttrs (
    map (
      world:
      let
        name = lib.removeSuffix ".json" world;
        report = fixtures.worldReport {
          world = "examples/unknown-world/${world}";
          inherit environment;
        };
      in
      lib.nameValuePair "example-unknown-${name}" (
        mkCheck "unknown-${name}" [ pkgs.python3 ] { } ''
          test "$(cat ${report}/exit-code)" = 0
          jq -e '.analysis.status == "Converged" and .analysis.frontiers == [] and (.analysis.procedure_summaries | length) > 0 and all(.analysis.procedure_summaries[]; .closed and (.procedures | length) > 0) and (.ssa | type == "object")' ${report}/report.json > /dev/null
          python ${validator} ${lib.escapeShellArg name} ${report}/report.json
          ln -s ${report}/report.json "$out/report.json"
        ''
      )
    ) fixtures.unknown
  );
}
