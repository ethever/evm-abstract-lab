# A single shared --ssa report contains both native analysis and SSA. Its raw
# projection serves the original CFG predicates without running analysis twice.
{
  lib,
  fixtures,
  mkCheck,
  ...
}:
{
  checks = lib.listToAttrs (
    map (
      world:
      let
        name = lib.removeSuffix ".json" world;
        report = fixtures.worldReport { world = "examples/worlds/${world}"; };
      in
      lib.nameValuePair "example-world-${name}" (
        mkCheck "world-${name}" [ ] { } ''
          test "$(cat ${report}/exit-code)" = 0
          jq -e '.status == "Converged" and .world.fork == "osaka" and any(.edges[]; .kind == "Call")' ${report}/analysis.json > /dev/null
          jq -e '.native.analysis.status == "Converged" and (.native.ssa | type == "object")' ${report}/report.json > /dev/null
          ln -s ${report}/analysis.json "$out/analysis.json"
          ln -s ${report}/report.json "$out/report.json"
        ''
      )
    ) fixtures.worlds
  );
}
