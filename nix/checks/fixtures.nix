# Installed world reports are shared inputs, rather than each consumer running
# the same abstract execution again. One --ssa invocation contains both the
# analysis and (when converged) SSA; incomplete worlds retain their raw report.
{
  pkgs,
  sourceRoot,
  build,
}:
let
  lib = pkgs.lib;
  inputs = import ../check-inputs.nix { inherit lib sourceRoot; };
  concrete = [
    "--evm.to"
    "0x0000000000000000000000000000000000000101"
    "--evm.caller"
    "0x0000000000000000000000000000000000001000"
    "--evm.value"
    "0"
    "--evm.calldata"
    "0x"
  ];
  worldReport =
    {
      world,
      variant ? "default",
      environment ? concrete,
      extraArgs ? [ ],
    }:
    let
      package =
        (if variant == "default" then build.release else build.imblRelease).packages.evm-abstract-cli;
      fixture = inputs.file world;
      command = [
        "${package}/bin/evm-abstract"
        "analyze"
        "--world"
        fixture
      ]
      ++ environment
      ++ [
        "--format"
        "json"
        "--ssa"
      ]
      ++ extraArgs;
    in
    # The name intentionally excludes the consumer: identical requests from
    # parity and example assertions must resolve to the same derivation.
    pkgs.runCommand "evm-abstract-world-report" { nativeBuildInputs = [ pkgs.python3 ]; } ''
      mkdir "$out"
      status=0
      ${lib.escapeShellArgs command} > "$out/report.json" 2> "$out/stderr" || status=$?
      case "$status" in 0|2) ;; *) cat "$out/stderr" >&2; exit "$status" ;; esac
      printf '%s\n' "$status" > "$out/exit-code"
      python - "$out" <<'PY'
      import json
      from pathlib import Path
      import sys
      root = Path(sys.argv[1])
      report = json.loads((root / "report.json").read_text())
      analysis = report.get("analysis", report)
      code = int((root / "exit-code").read_text())
      assert analysis["status"] == ("Converged" if code == 0 else "Incomplete")
      if code == 0:
          assert isinstance(report["ssa"], dict)
      else:
          assert "ssa" not in report
      (root / "analysis.json").write_text(json.dumps(analysis, indent=2) + "\n")
      PY
    '';
  jsonFiles =
    directory:
    lib.filter (name: lib.hasSuffix ".json" name) (
      builtins.attrNames (builtins.readDir (sourceRoot + "/${directory}"))
    );
in
{
  inherit inputs concrete worldReport;
  worlds = jsonFiles "examples/worlds";
  storage = lib.filter (name: lib.hasPrefix "storage-" name) (jsonFiles "examples");
  unknown = jsonFiles "examples/unknown-world";
}
