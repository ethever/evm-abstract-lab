//! Real native/projector coverage for the shared source directory and graph envelope.
use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{AnalysisInput, control::Control},
    domain::NumericValue,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use evm_abstract_protocol as api;

fn bytecode(code: &str) -> api::AnalyzeRequest {
    api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: Some(Address::repeat_byte(0x11).to_string()),
        }),
        ..Default::default()
    }
}

#[test]
fn immutable_catalog_is_shared_without_merging_source_accounts_or_graph_ids() {
    let root = Address::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "catalog identities");
    for address in [root, other] {
        world
            .insert(
                address,
                EvmAccount::from_hex("600160020100", Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    let report = evm_abstract_server::analyze::captured::analyze(
        AnalysisInput::new(world, EvmEntry::new(root)).unwrap(),
        &bytecode("600160020100"),
        &Control::default(),
    )
    .unwrap();
    report.validate().unwrap();
    assert_eq!(report.selected_view, api::AnalysisNamespace::Native);
    assert_eq!(report.programs.len(), 2);
    let root_program = report
        .programs
        .iter()
        .find(|program| program.code_address.as_deref() == Some(root.to_string().as_str()))
        .unwrap();
    let other_program = report
        .programs
        .iter()
        .find(|program| program.code_address.as_deref() == Some(other.to_string().as_str()))
        .unwrap();
    assert_eq!(root_program.code_hash, other_program.code_hash);
    assert_ne!(root_program.id, other_program.id);
    for namespace in [
        api::AnalysisNamespace::Baseline,
        api::AnalysisNamespace::Native,
    ] {
        let graph = report.graph(namespace).unwrap();
        assert_eq!(graph.metadata.root_program, Some(root_program.id));
        assert_eq!(report.disassembly(namespace).unwrap(), root_program.blocks);
        assert_eq!(graph.states[0].state, 0, "IDs are local to each graph");
    }
    let global = report.global.as_ref().unwrap();
    let native = report.native.as_ref().unwrap();
    for process in global.cfg.iter().filter(|block| block.process.is_some()) {
        assert!(process.executed_pcs.is_empty());
        assert!(process.source_block().is_none());
    }
    assert!(native.cfg[0].process.is_none());
    let block = &native.cfg[0];
    let source = report.source_block(block.source_block().unwrap()).unwrap();
    assert_eq!(
        source
            .instructions
            .iter()
            .map(|instruction| instruction.pc)
            .collect::<Vec<_>>(),
        [0, 2, 4, 5]
    );
    assert_eq!(block.executed_pcs, [0, 2, 4, 5]);
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("disassembly").is_none());
    for graph in [&json["global"], &json["native"]] {
        assert!(graph.get("programs").is_none());
        assert!(graph.get("disassembly").is_none());
        assert!(
            graph["cfg"]
                .as_array()
                .unwrap()
                .iter()
                .all(|block| block.get("instructions").is_none())
        );
    }
    let decoded: api::AnalysisReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, report);
}

#[test]
fn normalized_catalog_rejects_forged_sources_and_coverage_revisions() {
    let report = evm_abstract_server::analyze::analyze(bytecode("600160020100")).unwrap();
    report.validate().unwrap();
    let mut invalid = report.clone();
    invalid.programs.push(invalid.programs[0].clone());
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Source(
            api::SourceValidationKind::ProgramIdentity
        ))
    );
    let mut invalid = report.clone();
    std::sync::Arc::make_mut(&mut invalid.programs[0]).fork = api::Fork::Cancun;
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    std::sync::Arc::make_mut(invalid.native.as_mut().unwrap()).cfg[0].basic_block =
        Some(usize::MAX);
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    std::sync::Arc::make_mut(invalid.native.as_mut().unwrap()).cfg[0]
        .executed_pcs
        .push(1);
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Source(
            api::SourceValidationKind::Instruction
        ))
    );
    let mut invalid = report.clone();
    std::sync::Arc::make_mut(invalid.native.as_mut().unwrap())
        .ssa
        .blocks[0]
        .instructions[0]
        .opcode = 0x00;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Source(
            api::SourceValidationKind::Instruction
        ))
    );
    let mut invalid = report.clone();
    let hybrid = invalid.hybrid.as_mut().unwrap();
    hybrid.coverage.detail_revision = Some(api::DecimalU64(u64::MAX));
    for prefix in &mut hybrid.local_prefixes {
        prefix.revision = api::DecimalU64(u64::MAX);
    }
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
    let mut invalid = report.clone();
    invalid.hybrid.as_mut().unwrap().local_prefixes[0].namespace = api::AnalysisNamespace::Baseline;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Namespace
        ))
    );
    let mut invalid = report.clone();
    std::mem::swap(&mut invalid.global, &mut invalid.native);
    assert!(
        invalid.validate().is_err(),
        "equal numeric state IDs cannot cross namespaces"
    );
    let mut invalid = serde_json::to_value(&report).unwrap();
    invalid["native"]["cfg"][0]["instructions"] = serde_json::json!([]);
    assert!(serde_json::from_value::<api::AnalysisReport>(invalid).is_err());
}

#[test]
fn nonroot_identical_instructions_do_not_authorize_a_different_code_source() {
    let root = Address::repeat_byte(0x11);
    let callee = Address::repeat_byte(0x22);
    let other = Address::repeat_byte(0x33);
    let call = format!("5f5f5f5f5f73{callee:x}5af15000");
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "nonroot source binding");
    world
        .insert(root, EvmAccount::from_hex(&call, Fork::Osaka).unwrap())
        .unwrap();
    for address in [callee, other] {
        world
            .insert(
                address,
                EvmAccount::from_hex("600160020100", Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    let mut request = bytecode(&call);
    request.limits.use_summaries = false;
    request.limits.root_terminal_coverage = false;
    let report = evm_abstract_server::analyze::captured::analyze(
        AnalysisInput::new(world, EvmEntry::new(root)).unwrap(),
        &request,
        &Control::default(),
    )
    .unwrap();
    report.validate().unwrap();
    let native = report.native.as_ref().unwrap();
    let state = native
        .states
        .iter()
        .find(|state| {
            state.entry.frames.last().is_some_and(|frame| {
                frame.code_address.as_deref() == Some(callee.to_string().as_str())
            })
        })
        .unwrap()
        .state;
    let other_program = report
        .programs
        .iter()
        .find(|program| program.code_address.as_deref() == Some(other.to_string().as_str()))
        .unwrap()
        .id;
    let mut invalid = report.clone();
    let graph = std::sync::Arc::make_mut(invalid.native.as_mut().unwrap());
    graph
        .states
        .iter_mut()
        .find(|row| row.state == state)
        .unwrap()
        .entry
        .frames
        .last_mut()
        .unwrap()
        .program = Some(other_program);
    graph
        .cfg
        .iter_mut()
        .find(|row| row.id == state)
        .unwrap()
        .program = Some(other_program);
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Source(
            api::SourceValidationKind::Frame
        ))
    );
    let callee_program = native
        .states
        .iter()
        .find(|row| row.state == state)
        .unwrap()
        .entry
        .frames
        .last()
        .unwrap()
        .program
        .unwrap();
    let mut invalid = report.clone();
    std::sync::Arc::make_mut(
        invalid
            .programs
            .iter_mut()
            .find(|program| program.id == callee_program)
            .unwrap(),
    )
    .fork = api::Fork::Cancun;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Source(
            api::SourceValidationKind::Fork
        ))
    );
    let mut invalid = report;
    std::sync::Arc::make_mut(
        invalid
            .programs
            .iter_mut()
            .find(|program| program.id == callee_program)
            .unwrap(),
    )
    .code_hash = format!("0x{}", "ab".repeat(32));
    assert!(invalid.validate().is_err());
}

#[test]
fn partial_ssa_resolves_source_without_claiming_unexecuted_instructions() {
    let mut request = bytecode("60015f5100");
    request.limits.root_terminal_coverage = false;
    request.limits.max_memory_bytes = 1;
    let complete =
        evm_abstract_server::analyze::captured::native_bytecode(&request, &Control::default())
            .unwrap();
    let (mut minimum, mut maximum) = (1, complete.work);
    while minimum < maximum {
        let middle = minimum + (maximum - minimum) / 2;
        request.limits.max_work = middle;
        let candidate =
            evm_abstract_server::analyze::captured::native_bytecode(&request, &Control::default())
                .unwrap();
        if candidate
            .native
            .as_ref()
            .unwrap()
            .cfg
            .iter()
            .any(|block| block.executed_pcs.contains(&3))
        {
            maximum = middle;
        } else {
            minimum = middle + 1;
        }
    }
    request.limits.max_work = minimum;
    let report =
        evm_abstract_server::analyze::captured::native_bytecode(&request, &Control::default())
            .unwrap();
    report.validate().unwrap();
    let graph = report.native.as_ref().unwrap();
    assert_eq!(graph.status, api::AnalysisStatus::Incomplete);
    assert!(!graph.ssa.complete);
    assert!(!graph.frontiers.is_empty());
    let block = &graph.cfg[0];
    let source = report.source_block(block.source_block().unwrap()).unwrap();
    assert_eq!(
        source
            .instructions
            .iter()
            .map(|instruction| instruction.pc)
            .collect::<Vec<_>>(),
        [0, 2, 3, 4]
    );
    assert_eq!(block.executed_pcs, [0, 2, 3]);
    let load = graph.ssa.blocks[0]
        .instructions
        .iter()
        .find(|instruction| instruction.pc == 3)
        .unwrap();
    assert_eq!(load.progress, api::InstructionProgress::OperandsConsumed);
    assert!(load.results.is_empty());
    assert!(
        !graph.ssa.blocks[0]
            .instructions
            .iter()
            .any(|instruction| instruction.pc == 4)
    );
}
