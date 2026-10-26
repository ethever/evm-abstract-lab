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
        # Keep every original one-region request. These two lessons additionally
        # retain C's private source/joint oracle under the same execution budget.
        detailed = lib.elem name [
          "external-change"
          "outer-revert"
        ];
        detail = fixtures.worldReport {
          world = "examples/unknown-world/${world}";
          inherit environment;
          extraArgs = [
            "--max-detail-regions"
            "2"
          ];
        };
      in
      lib.nameValuePair "example-unknown-${name}" (
        mkCheck "unknown-${name}" [ pkgs.python3 ] { } ''
          test "$(cat ${report}/exit-code)" = 0
          jq -e '.native.analysis.status == "Converged" and .native.analysis.frontiers == [] and (.native.analysis.procedure_summaries | length) > 0 and all(.native.analysis.procedure_summaries[]; .closed and (.procedures | length) > 0) and (.native.ssa | type == "object")' ${report}/report.json > /dev/null
          ${lib.optionalString detailed ''test "$(cat ${detail}/exit-code)" = 0''}
          python ${validator} ${lib.escapeShellArg name} ${report}/report.json ${lib.optionalString detailed "--detail-report ${detail}/report.json"} --selftest
          ln -s ${report}/report.json "$out/report.json"
        ''
      )
    ) fixtures.unknown
  );
}
