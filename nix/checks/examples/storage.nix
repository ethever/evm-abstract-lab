# Storage worlds use the same concrete requests as backend parity. Symbolic
# calldata lessons have a different request and therefore a separate producer.
{
  lib,
  fixtures,
  concrete,
  mkCheck,
  ...
}:
let
  regular = lib.listToAttrs (
    map (
      world:
      let
        name = lib.removeSuffix ".json" world;
        report = fixtures.worldReport { world = "examples/${world}"; };
      in
      lib.nameValuePair "example-${name}" (
        mkCheck name [ ] { } ''
          test "$(cat ${report}/exit-code)" = 0
          jq -e '.analysis.status == "Converged" and (.ssa | type == "object")' ${report}/report.json > /dev/null
          ln -s ${report}/report.json "$out/report.json"
        ''
      )
    ) fixtures.storage
  );
  symbolic =
    name: predicate:
    let
      report = fixtures.worldReport {
        world = "examples/storage-symbolic-${name}.json";
        environment = lib.take ((builtins.length concrete) - 2) concrete;
      };
    in
    mkCheck "storage-symbolic-${name}-calldata" [ ] { } ''
      test "$(cat ${report}/exit-code)" = 0
      jq -e ${lib.escapeShellArg predicate} ${report}/analysis.json > /dev/null
      ln -s ${report}/analysis.json "$out/analysis.json"
    '';
in
{
  checks = regular // {
    example-storage-symbolic-key-calldata = symbolic "key" ''.status == "Converged" and .states[0].exit.call_stack.root.state.stack[0].Constants == ["0x7"] and .states[0].exit.call_stack.root.state.stack[0].provenance.origins.Sources == ["Storage"]'';
    example-storage-symbolic-value-calldata = symbolic "value" ''.status == "Converged" and .states[0].exit.call_stack.root.state.stack[0].Constants == ["0x1"]'';
  };
}
