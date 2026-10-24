# World-specific semantics consume the same reports as generic worlds/parity.
{
  pkgs,
  lib,
  fixtures,
  inputs,
  concrete,
  mkCheck,
  ...
}:
let
  known = lib.listToAttrs (
    map
      (
        reuse:
        let
          report = fixtures.worldReport {
            world = "examples/known-recursion/self-return-42.json";
            extraArgs = lib.optionals (reuse == "off") [ "--no-summaries" ];
          };
        in
        lib.nameValuePair "example-known-recursion-${reuse}" (
          mkCheck "known-recursion-${reuse}" [ ] { } ''
            test "$(cat ${report}/exit-code)" = 0
            # The canonical root carries semantics; the sparse byte cache is presentation.
            # Require the exact 32-byte word 42 without relaxing production precision.
            jq -e '
              def word42:
                try (
                  . as $graph | .nodes as $nodes |
                  def node($id):
                    if ($nodes | type) == "array" and ($id | type) == "number" and
                      $id >= 0 and $id == ($id | floor) and $id < ($nodes | length)
                    then $nodes[$id] else null end;
                  node($graph.root) as $slice |
                  node($slice.Slice.source) as $view |
                  node($view.View.state) as $write |
                  ($slice | keys) == ["Slice"] and
                  ($view | keys) == ["View"] and
                  ($write | keys) == ["WriteWord"] and
                  node($write.WriteWord.previous) != null and
                  node($view.View.offset) == {"Constant": "0x0"} and
                  node($view.View.length) == {"Constant": "0x20"} and
                  node($write.WriteWord.offset) == {"Constant": "0x0"} and
                  node($write.WriteWord.value) == {"Constant": "0x2a"}
                ) catch false;
              .hybrid.coverage.chosen == "Native" and
              .hybrid.coverage.baseline_current and .hybrid.coverage.detail_current and
              .analysis.schema_version == 15 and .analysis.status == "Converged" and
              .analysis.frontiers == [] and .ssa.complete and
              (.analysis.procedure_summaries | length) > 0 and
              all(.analysis.procedure_summaries[];
                .kind == "KnownRecursion" and .closed and
                (.root_procedure as $root | $root != null and
                  .procedures[$root].frame.mode == "Runtime" and
                  (.procedures[$root].covered_blocks | length) > 0)) and
              ([.analysis.states[].key.frames | length] | max) == 3 and
              any(.analysis.outcomes[]; .kind == "Return") and
              all(.analysis.outcomes[] | select(.kind == "Return");
                .data as $data | ($data | has("failure")) and
                $data.failure == null and $data.memory == false and
                $data.length.numeric.constants == ["0x20"] and ($data.root | word42))
            ' ${report}/report.json > /dev/null
            ln -s ${report}/report.json "$out/report.json"
          ''
        )
      )
      [
        "on"
        "off"
      ]
  );
  missing = mkCheck "missing-code-summary" [ ] { } ''
    ln -s ${
      fixtures.worldReport { world = "examples/worlds/missing-code.json"; }
    }/report.json missing.json
    jq -e '.analysis.status == "Converged" and .analysis.frontiers == [] and (.analysis.procedure_summaries | length) > 0 and all(.analysis.procedure_summaries[]; .closed) and (.ssa | type == "object")' missing.json > /dev/null
    cp -a missing.json "$out/"
  '';
  summary = mkCheck "summary-reuse" [ pkgs.python3 ] { } ''
    ln -s ${
      fixtures.worldReport { world = "examples/worlds/summary-reuse.json"; }
    }/analysis.json summary-on.json
    ln -s ${
      fixtures.worldReport {
        world = "examples/worlds/summary-reuse.json";
        extraArgs = [ "--no-summaries" ];
      }
    }/analysis.json summary-off.json
    jq -e '.status == "Converged" and .summary_stats.hits > 0 and .summary_stats.imported_states > 0 and any(.summaries[]; (.reused_at | length) > 0) and .world.identity.kind == "offline" and (.world.fingerprint | test("^0x[0-9a-f]{64}$"))' summary-on.json > /dev/null
    jq -e '.status == "Converged" and .summary_stats.hits == 0 and .summaries == []' summary-off.json > /dev/null
    # Producer namespaces are opaque across reports. Normalize the complete
    # reports bijectively before projecting, preserving all shared leaves.
    python3 ${inputs.file "scripts/normalize_identity_scopes.py"} < summary-on.json > normalized-on.json
    python3 ${inputs.file "scripts/normalize_identity_scopes.py"} < summary-off.json > normalized-off.json
    jq -S '[.outcomes[] | {kind,data,store}] | unique' normalized-on.json > on-relations.json
    jq -S '[.outcomes[] | {kind,data,store}] | unique' normalized-off.json > off-relations.json
    cmp on-relations.json off-relations.json
    cp -a summary-on.json summary-off.json "$out/"
  '';
  creation = mkCheck "creation" [ ] { } ''
    ln -s ${
      fixtures.worldReport { world = "examples/worlds/create-runtime.json"; }
    }/analysis.json creation.json
    jq -e 'any(.states[]; .key.frames[-1].mode == "InitCode") and any(.states[]; .key.frames[-1].mode == "Runtime" and .key.frames[-1].code_hash == "0x30962a84ef989ca0f724a5b2ec94f9cbf6a731752ce2c0be5333bf96e460c9fd") and any(.outcomes[]; .store.nonces["0x0000000000000000000000000000000000000101"].numeric.constants == ["0x1"] and any(.store.account_observations[]; .address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7" and .existence == "present" and .code_size == 8 and .nonce.numeric.constants == ["0x1"]))' creation.json > /dev/null
    cp -a creation.json "$out/"
  '';
  destruction = mkCheck "destruction" [ ] { } ''
    ln -s ${
      fixtures.worldReport { world = "examples/worlds/created-selfdestruct.json"; }
    }/analysis.json destruction.json
    jq -e 'any(.states[]; .entry.store.pending_destruction["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"] == true and .entry.store.codes["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"].Runtime.byte_len == 8) and any(.outcomes[]; .kind == "Return" and .store.balances["0x0000000000000000000000000000000000000200"].numeric.constants == ["0x7"] and any(.store.account_observations[]; .address == "0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb" and .existence == "absent" and .code_size == 0 and .nonce.numeric.constants == ["0x0"]))' destruction.json > /dev/null
    cp -a destruction.json "$out/"
  '';
  precompile = mkCheck "precompile" [ ] { } ''
    ln -s ${
      fixtures.worldReport { world = "examples/worlds/identity-precompile.json"; }
    }/analysis.json native.json
    # The complete output root is authoritative; byte caches are presentation.
    # A CALL failure may also return zero, so require a native success witness.
    jq -e '
      def word42:
        try (
          . as $graph |
          ($graph.identity_scope | type) == "string" and
          ($graph.identity_scope | length) > 0 and
          ($graph.nodes | type) == "array" and
          ($graph.root | type) == "number" and
          $graph.root >= 0 and $graph.root == ($graph.root | floor) and
          $graph.root < ($graph.nodes | length) and
          $graph.nodes[$graph.root] == {"BytesExact": ([range(31) | 0] + [42])}
        ) catch false;
      . as $report |
      {"Precompile": "0x0000000000000000000000000000000000000004"} as $native |
      .schema_version == 15 and .status == "Converged" and .frontiers == [] and
      any(.states[]; .key.frames[-1].mode == $native) and
      any(.outcomes[];
        . as $outcome |
        .kind == "Return" and (.data | has("failure")) and
        .data.failure == null and .data.memory == false and
        (.data.length | type) == "object" and
        .data.length.numeric.constants == ["0x20"] and (.data.root | word42) and
        # Bind this output to the caller buffer supplied by the native Return.
        any($report.states[];
          .id == $outcome.state and .key.frames[-1].mode == "Runtime" and
          .entry.call_stack.root.state.returndata.root == $outcome.data.root and
          any($report.edges[];
            .to == $outcome.state and .kind == "Return" and
            (.from as $from | any($report.states[];
              .id == $from and .key.frames[-1].mode == $native)))))
    ' native.json > /dev/null
    cp -a native.json "$out/"
  '';
in
{
  checks = known // {
    example-missing-code-summary = missing;
    example-summary-reuse = summary;
    example-creation = creation;
    example-destruction = destruction;
    example-precompile = precompile;
  };
  artifacts = [
    {
      name = "summary-on.json";
      path = "${summary}/summary-on.json";
    }
    {
      name = "summary-off.json";
      path = "${summary}/summary-off.json";
    }
    {
      name = "creation.json";
      path = "${creation}/creation.json";
    }
    {
      name = "destruction.json";
      path = "${destruction}/destruction.json";
    }
    {
      name = "native.json";
      path = "${precompile}/native.json";
    }
  ];
}
