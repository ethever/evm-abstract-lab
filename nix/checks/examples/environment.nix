# Environment defaults and explicit values each exercise installed CFG JSON.
{
  lib,
  concrete,
  mkCheck,
  ...
}:
{
  checks = {
    example-environment-symbolic = mkCheck "environment-symbolic" [ ] { } ''
      evm-abstract cfg --hex 33321400 --format json > symbolic-environment.json
      jq -e '
        .schema_version == 9 and .domain_spec.schema_version == 2 and
        .config.relations.enabled and (.config.relations | has("timeout_ms") | not) and
        .environment.to == {"Symbolic":"To"} and .environment.caller == {"Symbolic":"Caller"} and
        .environment.origin == null and .environment.value == "Top" and
        (.environment.calldata as $bytes | $bytes.length as $length |
          ($length | has("Constants") | not) and
          $length.known_bits == {"zero":"0x0","one":"0x0"} and
          $length.interval == {"unsigned_lo":"0x0","unsigned_hi":$max_word,"signed_lo":"0x0","signed_hi":$max_word} and
          $length.congruence == "Top" and $length.nonzero == false and
          $length.provenance == {"origins":"Top","code_address_role":false} and
          $bytes.bytes == {} and $bytes.memory == false and $bytes.failure == null and
          ($bytes.root.nodes[$bytes.root.root].BytesInput as $input |
            $input.bindings == [] and $input.source.namespace == "0x0" and
            $bytes.root.identity_scope == $length.expression.identity_scope and
            $bytes.root.nodes[$input.length] == $length.expression.nodes[$length.expression.root] and
            ($length.expression.nodes[$length.expression.root] | has("Fresh")))) and
        .states[0].exit_stack[0].Constants == ["0x1"]
      ' --arg max_word 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff symbolic-environment.json > /dev/null
      cp symbolic-environment.json "$out/report.json"
    '';
    example-environment-concrete = mkCheck "environment-concrete" [ ] { } ''
      evm-abstract cfg --hex 3033363446484a00 ${lib.escapeShellArgs concrete} --evm.chain-id 56 --evm.basefee 7 --evm.blob-basefee 8 --format json > concrete-environment.json
      jq -e '[.states[0].exit_stack[].Constants[0]] == ["0x101","0x1000","0x0","0x0","0x38","0x7","0x8"]' concrete-environment.json > /dev/null
      cp concrete-environment.json "$out/report.json"
    '';
  };
}
