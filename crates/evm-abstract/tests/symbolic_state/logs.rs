use super::support::{
    Assembly, Fixture, ROOT, address, complete, config, executed_opcode, possible_lengths,
    reachable, run, word,
};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{MachineOutcome, MachinePayload, OutcomeKind, WorldAnalysis, control::Control},
    domain::{
        AbstractValue, Domain,
        identity::Symbol,
        provenance::{Origin, Provenance},
        relational::{CheckResult, ValueQuery},
        symbolic::{ByteStateKind, ExprError, ExprKind, GraphLimits, SymbolicGraph},
    },
    render::{
        json::{JsonError, JsonLimits, with_control},
        world::evidence,
    },
    world::AbstractLog,
};
use revm_bytecode::opcode;
use serde_json::{Value, json};

fn log_fixture() -> Fixture {
    let mut code = Assembly::default();
    // EVM pop order is (L, bound). These guards admit an unenumerated range,
    // and the word guard belongs to the original unknown calldata source.
    code.push(128)
        .op(opcode::CALLDATASIZE)
        .op(opcode::GT)
        .jump("outside", true)
        .push(32)
        .op(opcode::CALLDATASIZE)
        .op(opcode::LT)
        .jump("outside", true)
        .load_input(0)
        .push(7)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("outside", true)
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .op(opcode::DUP1)
        .label("copy")
        .op(opcode::CALLDATACOPY)
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .label("log")
        .op(opcode::LOG0)
        // The block boundary exposes the real pre-overwrite snapshot, instead
        // of reconstructing an AbstractLog by hand after execution.
        .label("overwrite")
        .push(9)
        .op(opcode::PUSH0)
        .label("write")
        .op(opcode::MSTORE)
        .label("done")
        .op(opcode::STOP)
        .label("outside")
        .op(opcode::STOP);
    code.finish()
}

fn captured_log<'a>(
    analysis: &'a WorldAnalysis,
    fixture: &Fixture,
) -> (&'a MachineOutcome, &'a AbstractLog, &'a MachinePayload) {
    complete(analysis);
    // The engine retains one conservative entry gas failure independently of
    // executed instructions (engine.rs). It must leave the whole store intact;
    // every executed route in this fixture completes normally.
    let failures: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind != OutcomeKind::Return)
        .collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].kind, OutcomeKind::Failure);
    assert_eq!(failures[0].state, 0);
    assert_eq!(failures[0].data.len().singleton(), Some(U256::ZERO));
    assert_eq!(
        serde_json::to_value(&failures[0].store).unwrap(),
        serde_json::to_value(&analysis.states()[0].entry.store).unwrap()
    );
    let log_pc = executed_opcode(analysis, fixture, "log", opcode::LOG0);
    executed_opcode(analysis, fixture, "copy", opcode::CALLDATACOPY);
    executed_opcode(analysis, fixture, "write", opcode::MSTORE);
    executed_opcode(analysis, fixture, "done", opcode::STOP);
    assert!(reachable(analysis, ROOT, fixture.labels["outside"]));
    assert!(
        analysis.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return && outcome.store.possible_logs().is_empty()
        }),
        "normal outside completion must survive"
    );
    let logged: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| {
            outcome.kind == OutcomeKind::Return && !outcome.store.possible_logs().is_empty()
        })
        .collect();
    assert_eq!(logged.len(), 1, "retain the successful symbolic LOG route");
    let outcome = logged[0];
    assert_eq!(outcome.data.len().singleton(), Some(U256::ZERO));
    assert_eq!(outcome.store.possible_logs().len(), 1);
    assert!(!outcome.store.logs_unknown());
    let (key, log) = outcome.store.possible_logs().iter().next().unwrap();
    assert_eq!(key.address, address(ROOT));
    assert_eq!(key.code_address, address(ROOT));
    assert_eq!(key.pc, log_pc);
    assert!(log.topics.is_empty());
    let before = analysis
        .states()
        .iter()
        .find(|state| state.executed_pcs.contains(&log_pc))
        .unwrap()
        .exit
        .as_ref()
        .unwrap();
    (outcome, log, before)
}

fn verify_history(analysis: &WorldAnalysis, fixture: &Fixture) -> (AbstractValue, AbstractValue) {
    let (outcome, log, before) = captured_log(analysis, fixture);
    let domain = Domain::from_spec(analysis.domain_spec());
    let payload = analysis.states()[outcome.state].exit.as_ref().unwrap();
    let memory = &payload.active().memory;
    let calldata = &before.active().calldata;
    possible_lengths(&payload.relations, log.data.len(), &[32, 33, 64, 128]);
    for length in [0, 31, 129] {
        assert_eq!(
            payload.relations.can_equal(
                log.data.len().expression().unwrap(),
                U256::from(length),
                &config().analysis.relations
            ),
            CheckResult::Unsat
        );
    }
    // Compare every retained product/source field with the real pre-write slice.
    let length = log.data.len().expression().unwrap();
    let copied_length_fact = before
        .active()
        .memory
        .symbolic_facts()
        .find(|(expression, _)| *expression == length)
        .unwrap()
        .1;
    let captured = before
        .active()
        .memory
        .slice(
            &word(0),
            copied_length_fact,
            config().max_memory_bytes,
            domain,
        )
        .unwrap();
    assert_eq!(log.data, captured);
    assert_eq!(log.data.len(), copied_length_fact);
    let expected_length = domain
        .reduce(&AbstractValue::unsigned_range(U256::from(32), U256::from(128)).unwrap())
        .value;
    assert_eq!(log.data.len().numeric(), expected_length.numeric());
    assert_eq!(
        log.data.len().provenance(),
        &Provenance::source(Origin::Calldata)
    );
    assert_eq!(log.data.len().expression(), calldata.len().expression());
    let ExprKind::Input(atom) = length.kind() else {
        panic!("length must keep its scoped environment input");
    };
    assert_eq!(atom.symbol, Symbol::CalldataLength);
    let ByteStateKind::Copy {
        previous,
        destination,
        source,
        length: copied_length,
    } = before.active().memory.state_root().kind()
    else {
        panic!("LOG must capture memory after the actual CALLDATACOPY");
    };
    assert!(matches!(previous.kind(), ByteStateKind::Zero));
    assert_eq!(destination.as_constant(), Some(U256::ZERO));
    assert_eq!(source.state, *calldata.state_root());
    assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
    assert_eq!(&source.length, length);
    assert_eq!(copied_length, length);
    // Complete copy/slice canonicalization may recover Input directly. Prove
    // that binding and the actual ordered history separately.
    assert_eq!(log.data.state_root(), calldata.state_root());
    let ByteStateKind::Input {
        source: captured_source,
        length: initial_length,
        bindings,
    } = log.data.state_root().kind()
    else {
        panic!("the complete captured input must keep its source binding");
    };
    assert_eq!(initial_length, length);
    let ByteStateKind::Input {
        source: initial_source,
        length: entry_length,
        bindings: entry_bindings,
    } = analysis.states()[0]
        .entry
        .active()
        .calldata
        .state_root()
        .kind()
    else {
        panic!("initial calldata must be a scoped source");
    };
    assert_eq!(captured_source, initial_source);
    assert_eq!(initial_length, entry_length);
    assert_eq!(bindings, entry_bindings);
    assert_eq!(captured_source.namespace(), U256::ZERO);
    // The source array is keyed separately from scalar words. Bound calldata
    // intentionally shares its opaque source token with the length Input atom.
    assert_eq!(captured_source.token(), length);
    let ByteStateKind::WriteWord {
        previous,
        offset,
        value,
    } = memory.state_root().kind()
    else {
        panic!("the later MSTORE must retain the old memory root");
    };
    assert_eq!(previous, before.active().memory.state_root());
    assert_eq!(offset.as_constant(), Some(U256::ZERO));
    assert_eq!(value.as_constant(), Some(U256::from(9)));
    assert_ne!(memory.state_root(), log.data.state_root());
    let old = log.data.read_word_checked(&word(0), domain).unwrap();
    let initial = calldata.read_word_checked(&word(0), domain).unwrap();
    assert_eq!(old.numeric(), initial.numeric());
    assert_eq!(old.provenance(), initial.provenance());
    assert_eq!(old.expression(), initial.expression());
    assert_eq!(old, log.data.read_word_checked(&word(0), domain).unwrap());
    let current = memory.read_word_checked(&word(0), domain).unwrap();
    assert_eq!(current.numeric(), word(9).numeric());
    assert_eq!(current.provenance(), word(9).provenance());
    for (read, expected) in [(&old, 7), (&current, 9)] {
        assert_eq!(
            payload.relations.unique_value(
                &read.symbolic_expression().unwrap(),
                &config().analysis.relations
            ),
            ValueQuery::Unique(U256::from(expected))
        );
    }
    (old, current)
}

#[test]
fn symbolic_length_log_keeps_the_copied_input_before_memory_overwrite() {
    let fixture = log_fixture();
    eprintln!(
        "symbolic LOG fixture={} labels={:?}",
        fixture.code, fixture.labels
    );
    let analysis = run(&fixture.code);
    verify_history(&analysis, &fixture);
}

fn graph_root(graph: &Value) -> &Value {
    &graph["nodes"][usize::try_from(graph["root"].as_u64().unwrap()).unwrap()]
}

fn source_binding(graph: &Value, input: &Value) -> Value {
    let token = usize::try_from(input["source"]["token"].as_u64().unwrap()).unwrap();
    json!([
        graph["identity_scope"],
        input["source"]["namespace"],
        graph["nodes"][token]
    ])
}

#[test]
fn symbolic_log_native_graphs_and_bounded_report_preserve_all_bindings() {
    let fixture = log_fixture();
    let analysis = run(&fixture.code);
    let (old, current) = verify_history(&analysis, &fixture);
    let (outcome, log, _) = captured_log(&analysis, &fixture);
    let limits = GraphLimits::default();
    let view = log.data.view();
    let view_graph = serde_json::to_value(SymbolicGraph::view(&view, limits).unwrap()).unwrap();
    let read_graph =
        serde_json::to_value(SymbolicGraph::expression(old.expression().unwrap(), limits).unwrap())
            .unwrap();
    let length_graph = serde_json::to_value(
        SymbolicGraph::expression(log.data.len().expression().unwrap(), limits).unwrap(),
    )
    .unwrap();
    let root_graph =
        serde_json::to_value(SymbolicGraph::bytes(log.data.state_root(), limits).unwrap()).unwrap();
    let native = serde_json::to_value(log).unwrap();
    assert_eq!(native["data"]["root"], root_graph);
    assert_eq!(
        native["data"]["length"],
        serde_json::to_value(log.data.len()).unwrap()
    );
    assert_eq!(native["data"]["length"]["expression"], length_graph);
    let input = &graph_root(&root_graph)["BytesInput"];
    let token = usize::try_from(input["source"]["token"].as_u64().unwrap()).unwrap();
    let source_leaf = &root_graph["nodes"][token];
    assert!(source_leaf.get("Fresh").is_some() || source_leaf.get("Input").is_some());
    assert_eq!(input["source"]["namespace"], json!(U256::ZERO));
    let length_node = usize::try_from(input["length"].as_u64().unwrap()).unwrap();
    assert_eq!(root_graph["nodes"][length_node], *graph_root(&length_graph));
    for graph in [&view_graph, &read_graph, &length_graph] {
        assert_eq!(graph["identity_scope"], root_graph["identity_scope"]);
    }
    for graph in [&view_graph, &read_graph] {
        assert!(graph["nodes"].as_array().unwrap().contains(source_leaf));
        assert!(
            graph["nodes"]
                .as_array()
                .unwrap()
                .contains(graph_root(&length_graph))
        );
    }
    let exported_view = &graph_root(&view_graph)["View"];
    let state_index = usize::try_from(exported_view["state"].as_u64().unwrap()).unwrap();
    let view_input = &view_graph["nodes"][state_index]["BytesInput"];
    assert_eq!(
        source_binding(&view_graph, view_input),
        source_binding(&root_graph, input)
    );
    let view_length = usize::try_from(exported_view["length"].as_u64().unwrap()).unwrap();
    assert_eq!(view_graph["nodes"][view_length], *graph_root(&length_graph));
    let operands = native["data"]["operands"].as_array().unwrap();
    assert_eq!(operands.len(), log.data.symbolic_facts().count());
    assert!(!operands.is_empty());
    for (expression, fact) in log.data.symbolic_facts() {
        let expression_json = serde_json::to_value(expression).unwrap();
        let operand = operands
            .iter()
            .find(|operand| operand["expression"] == expression_json)
            .unwrap();
        assert_eq!(operand["value"], serde_json::to_value(fact).unwrap());
        assert_eq!(
            operand["expression"]["identity_scope"],
            root_graph["identity_scope"]
        );
    }
    // The complete related native values round-trip, including full product and
    // provenance, not just a graph stripped of observations.
    let captured =
        json!({"log": native, "view": view_graph, "old_read": old, "current_read": current});
    assert_eq!(
        serde_json::from_slice::<Value>(&serde_json::to_vec(&captured).unwrap()).unwrap(),
        captured
    );

    let report = evidence(&analysis).unwrap();
    let encoded = with_control(&report, JsonLimits::default(), &Control::default()).unwrap();
    let decoded: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        decoded["schema_version"],
        json!(evm_abstract::analysis::SCHEMA_VERSION)
    );
    let outcome_index = analysis
        .outcomes()
        .iter()
        .position(|candidate| std::ptr::eq(candidate, outcome))
        .unwrap();
    let exported = &decoded["outcomes"][outcome_index]["store"]["possible_logs"][0];
    assert_eq!(exported["data"], captured["log"]["data"]);
    assert_eq!(exported["topics"], captured["log"]["topics"]);
    assert_eq!(exported["address"], json!(address(ROOT)));
    assert_eq!(exported["code_address"], json!(address(ROOT)));
    assert_eq!(exported["pc"], json!(fixture.labels["log"] + 1));
    let exported_length = &exported["data"]["length"]["expression"];
    assert_eq!(graph_root(exported_length), graph_root(&length_graph));
    assert_eq!(
        exported_length["identity_scope"],
        root_graph["identity_scope"]
    );
    let initial_root =
        &decoded["states"][0]["entry"]["call_stack"]["root"]["state"]["calldata"]["root"];
    assert_eq!(
        source_binding(initial_root, &graph_root(initial_root)["BytesInput"]),
        source_binding(&root_graph, input)
    );
    let memory = &analysis.states()[outcome.state]
        .exit
        .as_ref()
        .unwrap()
        .active()
        .memory;
    let exported_memory =
        &decoded["states"][outcome.state]["exit"]["call_stack"]["root"]["state"]["memory"];
    assert_eq!(*exported_memory, serde_json::to_value(memory).unwrap());
    let memory_graph = &exported_memory["root"];
    let history_inputs: Vec<_> = memory_graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node.get("BytesInput"))
        .collect();
    assert!(!history_inputs.is_empty());
    for history_input in history_inputs {
        assert_eq!(
            source_binding(memory_graph, history_input),
            source_binding(&root_graph, input)
        );
        let length_index = usize::try_from(history_input["length"].as_u64().unwrap()).unwrap();
        assert_eq!(
            memory_graph["nodes"][length_index],
            *graph_root(exported_length)
        );
    }
    // Separate per-graph retention from total report bytes. Each failure must
    // leave the same captured LOG, history, numeric facts and source identities.
    assert!(matches!(
        SymbolicGraph::view(
            &view,
            GraphLimits {
                max_nodes: 1,
                ..limits
            }
        ),
        Err(ExprError::Limit { .. })
    ));
    assert!(
        matches!(with_control(&report, JsonLimits { max_bytes: 128 }, &Control::default()), Err(JsonError::Bytes { required, limit: 128, written }) if required > 128 && written <= 128)
    );
    let byte_limit = encoded.len() - 1;
    assert!(
        matches!(with_control(&report, JsonLimits { max_bytes: byte_limit }, &Control::default()), Err(JsonError::Bytes { required, limit, written }) if required > limit && limit == byte_limit && written <= limit)
    );
    let cancellation = Control::new(Default::default());
    cancellation.cancellation().cancel();
    cancellation.scope(|| {
        assert!(matches!(
            SymbolicGraph::view(&view, limits),
            Err(ExprError::Cancelled)
        ));
    });
    assert!(matches!(
        with_control(&report, JsonLimits::default(), &cancellation),
        Err(JsonError::Cancelled { written: 0 })
    ));
    assert_eq!(serde_json::to_value(log).unwrap(), captured["log"]);
    assert_eq!(
        serde_json::from_str::<Value>(
            &with_control(&report, JsonLimits::default(), &Control::default()).unwrap()
        )
        .unwrap(),
        decoded
    );
    assert_eq!(verify_history(&analysis, &fixture), (old, current));
    eprintln!(
        "symbolic LOG native evidence: state={} pc={} report_bytes={} operand_facts={} identity_scope={} source_binding={}",
        outcome.state,
        fixture.labels["log"] + 1,
        encoded.len(),
        operands.len(),
        root_graph["identity_scope"],
        source_binding(&root_graph, input)
    );
}
