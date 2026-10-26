# Presentation formats deliberately execute their installed CLI paths. JSON
# consumers above share world reports; text/DOT are separate format acceptance.
{
  pkgs,
  lib,
  inputs,
  concrete,
  mkCheck,
  fontConfig,
  ...
}:
let
  summary-text = mkCheck "summary-text" [ ] { } ''
    evm-abstract analyze --world ${inputs.file "examples/worlds/summary-reuse.json"} ${lib.escapeShellArgs concrete} > summary.txt
    grep -q 'Detail regions: 1 requested' summary.txt
    grep -Eq 'KnownCall .*source_current=true .*closed=true .*reused=[1-9][0-9]*' summary.txt
    grep -q 'fingerprint=0x' summary.txt
    # The original full-cache presentation remains a separate precision case.
    evm-abstract analyze --world ${inputs.file "examples/worlds/summary-reuse.json"} ${lib.escapeShellArgs concrete} --max-detail-regions 2 > summary-native-cache.txt
    grep -Eq 'hits=[1-9][0-9]*' summary-native-cache.txt
    grep -q 'reused_at=' summary-native-cache.txt
    grep -q 'fingerprint=0x' summary-native-cache.txt
    cp summary.txt summary-native-cache.txt "$out/"
  '';
  summaries-dot = mkCheck "summaries-dot" [ pkgs.graphviz ] { FONTCONFIG_FILE = fontConfig; } ''
    evm-abstract analyze --world ${inputs.file "examples/worlds/summary-reuse.json"} ${lib.escapeShellArgs concrete} --format dot > summaries.dot
    dot -Tsvg summaries.dot -o summaries.svg
    test -s summaries.svg
    grep -q 'Procedure relation .*KnownCall' summaries.dot
    grep -Eq 'closed=true .*reused=[1-9][0-9]*' summaries.dot
    evm-abstract analyze --world ${inputs.file "examples/worlds/summary-reuse.json"} ${lib.escapeShellArgs concrete} --max-detail-regions 2 --format dot > summaries-native-cache.dot
    dot -Tsvg summaries-native-cache.dot -o summaries-native-cache.svg
    test -s summaries-native-cache.svg
    grep -q 'label="reused"' summaries-native-cache.dot
    cp summaries.dot summaries.svg summaries-native-cache.dot summaries-native-cache.svg "$out/"
  '';
  creation-dot = mkCheck "creation-dot" [ pkgs.graphviz ] { FONTCONFIG_FILE = fontConfig; } ''
    evm-abstract analyze --world ${inputs.file "examples/worlds/create-runtime.json"} ${lib.escapeShellArgs concrete} --format dot > creation.dot
    dot -Tsvg creation.dot -o creation.svg
    test -s creation.svg
    cp creation.dot creation.svg "$out/"
  '';
  proxies-dot = mkCheck "proxies-dot" [ pkgs.graphviz ] { FONTCONFIG_FILE = fontConfig; } ''
    evm-abstract analyze --world ${inputs.file "examples/worlds/proxy-storage.json"} ${lib.escapeShellArgs concrete} --format dot > proxies.dot
    dot -Tsvg proxies.dot -o proxies.svg
    test -s proxies.svg
    cp proxies.dot proxies.svg "$out/"
  '';
  diamond-dot = mkCheck "diamond-dot" [ pkgs.graphviz ] { FONTCONFIG_FILE = fontConfig; } ''
    evm-abstract cfg --file ${inputs.file "examples/diamond.hex"} --format dot > diamond.dot
    dot -Tsvg diamond.dot -o diamond.svg
    test -s diamond.svg
    cp diamond.dot diamond.svg "$out/"
  '';
in
{
  checks = {
    example-summary-text = summary-text;
    example-summaries-dot = summaries-dot;
    example-creation-dot = creation-dot;
    example-proxies-dot = proxies-dot;
    example-diamond-dot = diamond-dot;
  };
  artifacts = [
    {
      name = "summary.txt";
      path = "${summary-text}/summary.txt";
    }
    {
      name = "summaries.dot";
      path = "${summaries-dot}/summaries.dot";
    }
    {
      name = "summaries.svg";
      path = "${summaries-dot}/summaries.svg";
    }
    {
      name = "creation.dot";
      path = "${creation-dot}/creation.dot";
    }
    {
      name = "creation.svg";
      path = "${creation-dot}/creation.svg";
    }
    {
      name = "proxies.dot";
      path = "${proxies-dot}/proxies.dot";
    }
    {
      name = "proxies.svg";
      path = "${proxies-dot}/proxies.svg";
    }
    {
      name = "diamond.dot";
      path = "${diamond-dot}/diamond.dot";
    }
    {
      name = "diamond.svg";
      path = "${diamond-dot}/diamond.svg";
    }
  ];
}
