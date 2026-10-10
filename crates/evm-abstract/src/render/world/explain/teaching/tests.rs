use evm_abstract_notation::Symbol;

use super::render;
use crate::{
    Fork,
    analysis::{
        Diagnostic, DiagnosticKind, ExecutionConfig, FrameCode, Status, WorldAnalysis,
        analyze_world,
    },
    domain::{AbstractValue, Domain, Profile, ReductionStatus},
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256, hex, keccak256};

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn entry(owner: Address, calldata: ByteArray) -> Entry {
    Entry::concrete(
        owner,
        address(0x900),
        AbstractValue::constant(U256::ZERO),
        calldata,
    )
}

fn run(world: World, owner: Address, calldata: ByteArray) -> WorldAnalysis {
    analyze_world(world, entry(owner, calldata), ExecutionConfig::default()).unwrap()
}

fn simple(code: &str, calldata: ByteArray) -> WorldAnalysis {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "teaching fixture");
    world
        .insert(owner, Account::from_hex(code, Fork::Osaka).unwrap())
        .unwrap();
    run(world, owner, calldata)
}

fn section<'a>(output: &'a str, title: &str, next: &str) -> &'a str {
    output
        .split_once(&format!("\n{title}\n"))
        .unwrap()
        .1
        .split_once(&format!("\n{next}\n"))
        .unwrap()
        .0
}

#[test]
fn default_teaching_view_joins_code_cfg_results_and_instruction_ssa() {
    let analysis = simple("602a5f5260205ff3", ByteArray::empty());
    let output = render(&analysis).unwrap();
    assert!(output.contains("status=Converged"));
    assert!(output.contains("Instruction lists are syntactic"));
    assert!(output.contains("σ₀ | B₀ @ 0x0000 | f₀ active | code=C₀"));
    assert!(output.contains("stack in  []"));
    assert!(output.contains("stack out []"));
    assert!(
        output.contains("| Return | returndata length={0x20} (32 bytes) | word=0x2a"),
        "{output}"
    );
    assert!(output.contains("Verified cross-contract SSA:"));
    assert!(output.contains("= PUSH1 0x2a"));
    assert!(
        output.contains("complete storage, balances, lifecycle, logs and byte facts: --verbose")
    );
    for raw_field in [
        "captured frames:",
        "State details",
        "Account observations",
        "stored byte facts",
        "machine code identity",
        "instruction effects:",
        "opcode=",
        "immediate=",
        "fault=",
    ] {
        assert!(
            !output.contains(raw_field),
            "unexpected verbose field {raw_field}"
        );
    }
    assert_eq!(render(&analysis).unwrap(), output);
}

#[test]
fn overlay_runtime_and_initcode_use_actual_captured_programs_once() {
    let creator = address(0x101);
    let deployed = creator.create(0);
    let runtime = hex::decode("602a5f5260205ff3").unwrap();
    let mut initcode = hex::decode("61000861000d5f396100085ff3").unwrap();
    initcode.extend(&runtime);
    let mut factory =
        hex::decode("6100156100265f396100155f5ff0805f555060205f5f5f5f5f5462fffffff160015560205ff3")
            .unwrap();
    factory.extend(&initcode);
    let mut world = World::new(Fork::Osaka, "created captured code");
    world
        .insert(
            creator,
            Account::from_hex(&hex::encode(&factory), Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(deployed, Account::absent()).unwrap();
    world.insert(Address::ZERO, Account::absent()).unwrap();
    let analysis = run(world, creator, ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    assert_eq!(
        analysis.world().raw_account_code(deployed),
        Some(Vec::new())
    );
    assert!(analysis.states().iter().any(|state| {
        state.active().mode == FrameCode::Runtime && state.active().code_address == deployed
    }));

    let output = render(&analysis).unwrap();
    let codes = section(&output, "Execution code", "CFG");
    assert!(codes.contains("mode=InitCode"));
    assert!(codes.contains("mode=Runtime"));
    assert!(codes.contains("Captured instruction list (8 bytes)"));
    assert!(codes.contains("PUSH1          0x2a"));
    assert_eq!(codes.matches("Captured instruction list").count(), 3);
    for bytes in [&factory, &initcode, &runtime] {
        assert_eq!(output.matches(&keccak256(bytes).to_string()).count(), 1);
    }
    assert!(!output.contains("Input code observations"));
}

#[test]
fn shared_delegate_code_has_one_catalogue_entry_and_distinct_state_owners() {
    let owner = address(0x101);
    let implementation = address(0x200);
    let mut world = World::new(Fork::Osaka, "delegate owners");
    world
        .insert(
            owner,
            Account::from_hex("5f5f5f5f6102005af4505f5f5f5f5f6102005af15000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            implementation,
            Account::from_hex("5f00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let analysis = run(world, owner, ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    let codes = section(&output, "Execution code", "CFG");
    assert_eq!(codes.matches("Captured instruction list").count(), 2);
    assert!(codes.contains("state owners: A₀, A₁"));
    assert!(section(&output, "CFG", "Outcomes").contains("code=C₁ | state owner=A₀"));
    assert_eq!(output.matches(&owner.to_string()).count(), 1);
    assert_eq!(output.matches(&implementation.to_string()).count(), 1);
}

#[test]
fn equal_returned_bytes_keep_distinct_outcomes_and_storage_facts() {
    let analysis = simple("5f35600a5760015f55005b60025f5500", ByteArray::unknown());
    assert_eq!(analysis.status(), Status::Converged);
    let returns = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == crate::analysis::OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert_eq!(returns.len(), 2);
    assert_eq!(returns[0].data, returns[1].data);
    let output = render(&analysis).unwrap();
    let outcomes = section(&output, "Outcomes", "Diagnostics");
    for (index, outcome) in analysis.outcomes().iter().enumerate() {
        assert!(outcomes.contains(&format!(
            "{} | {} | {:?}",
            Symbol::Outcome(index),
            Symbol::State(outcome.state),
            outcome.kind
        )));
    }
    assert!(outcomes.contains("A₀[0x0]={0x1}"));
    assert!(outcomes.contains("A₀[0x0]={0x2}"));
    assert!(outcomes.contains("equal returndata does not merge outcomes"));
}

#[test]
fn root_revert_retains_its_data_and_rollback_ssa() {
    let analysis = simple("60075f55602a5f5260205ffd", ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    assert!(
        output.contains("| Revert | returndata length={0x20} (32 bytes) | word=0x2a"),
        "{output}"
    );
    assert!(output.contains("abstract explicit storage changes: (none)"));
    assert!(output.contains("REVERT %"));
}

#[test]
fn domain_projection_does_not_report_unchanged_storage_or_balance_as_effects() {
    let owner = address(0x101);
    let domain = Domain::new(std::num::NonZeroUsize::new(4).unwrap());
    let initial = domain.join(
        &AbstractValue::constant(U256::from(1)),
        &AbstractValue::constant(U256::from(3)),
    );
    let mut account = Account::from_hex("00", Fork::Osaka).unwrap();
    account.storage.insert(U256::ZERO, initial.clone());
    account.balance = initial;
    let mut world = World::new(Fork::Osaka, "initial domain projection");
    world.insert(owner, account).unwrap();
    let mut config = ExecutionConfig::default();
    config.analysis.domain_profile = Profile::ConstantsOnly;
    config.analysis.max_constants = 1;
    let analysis = analyze_world(world, entry(owner, ByteArray::empty()), config).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    assert!(
        output.contains("abstract explicit storage changes: (none)"),
        "{output}"
    );
    assert!(!output.contains("changed balances:"), "{output}");
}

#[test]
fn unknown_return_bytes_keep_length_default_and_detail_location() {
    let analysis = simple("5f355f5260205ff3", ByteArray::unknown());
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.data.exact_bytes().is_none())
    );
    let output = render(&analysis).unwrap();
    assert!(output.contains("returndata length={0x20} (32 bytes)"));
    assert!(output.contains("abstract or long bytes | default byte="));
    assert!(output.contains("full byte facts: --verbose"));
    assert!(!output.contains("| word="));
}

#[test]
fn converged_unknown_jump_and_fact_caps_remain_visible() {
    let mut analysis = simple("5f35565b00", ByteArray::unknown());
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.kind == DiagnosticKind::UnknownJump })
    );
    // 渲染器保留记录本身，不因收敛或多条同位置诊断而筛掉精度边界。
    analysis.diagnostics.extend([
        Diagnostic {
            state: 0,
            pc: 1,
            kind: DiagnosticKind::OpaqueResult,
        },
        Diagnostic {
            state: 0,
            pc: 1,
            kind: DiagnosticKind::FactExchangeLimited(ReductionStatus::FactLimit),
        },
    ]);
    let output = render(&analysis).unwrap();
    let diagnostics = section(&output, "Diagnostics", "Frontiers");
    assert_eq!(diagnostics.lines().count(), analysis.diagnostics().len());
    for diagnostic in analysis.diagnostics() {
        assert!(diagnostics.contains(&format!(
            "{} @ 0x{:04x}: {:?}",
            Symbol::State(diagnostic.state),
            diagnostic.pc,
            diagnostic.kind
        )));
    }
    assert!(output.contains("fact atoms=256"));
    assert!(output.contains("Verified cross-contract SSA:"));
}

#[test]
fn partial_native_and_missing_code_have_every_frontier_without_fake_ssa() {
    let native = run(
        World::new(Fork::Osaka, "native boundary"),
        address(4),
        ByteArray::unknown(),
    );
    let missing = run(
        World::new(Fork::Osaka, "missing code boundary"),
        address(0x101),
        ByteArray::empty(),
    );
    for analysis in [&native, &missing] {
        assert_eq!(analysis.status(), Status::Incomplete);
        let output = render(analysis).unwrap();
        assert!(output.contains("\nCFG\n"));
        for (index, frontier) in analysis.frontiers().iter().enumerate() {
            assert!(output.contains(&format!("{} | from=", Symbol::Frontier(index))));
            assert!(output.contains(&format!("reason={:?}", frontier.reason)));
            if let Some(target) = &frontier.target {
                for (frame, key) in target.frames.iter().enumerate() {
                    assert!(output.contains(&format!(
                        "{} | {}",
                        Symbol::Frame(frame),
                        Symbol::Block(key.basic_block_index)
                    )));
                    assert!(output.contains(&format!("context={:?}", key.jump_history)));
                }
            }
        }
        assert!(output.ends_with("SSA unavailable: cross-contract frontiers remain\n"));
        assert!(!output.contains("Verified cross-contract SSA:"));
    }
    let native_output = render(&native).unwrap();
    assert!(native_output.contains("Native precompile"));
    assert!(!native_output.contains("Captured instruction list"));
    assert!(!native_output.contains("B₀ @"));
}

#[test]
fn zero_state_work_boundary_preserves_observed_code_without_execution_claim() {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "initial budget boundary");
    world
        .insert(owner, Account::from_hex("602a00", Fork::Osaka).unwrap())
        .unwrap();
    let analysis = analyze_world(
        world,
        entry(owner, ByteArray::empty()),
        ExecutionConfig {
            max_work: 1,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.states().is_empty());
    let output = render(&analysis).unwrap();
    assert!(output.contains("Input code observations (not execution evidence)"));
    assert!(output.contains("Observed instruction list (execution not established)"));
    assert!(output.contains("PUSH1          0x2a"));
    assert!(!output.contains("Captured instruction list"));
}

#[test]
fn partial_runtime_keeps_code_and_cfg_without_a_successful_call_guess() {
    let analysis = simple("5f5f5f5f5f5f355af1", ByteArray::unknown());
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(!analysis.states().is_empty());
    let output = render(&analysis).unwrap();
    assert!(output.contains("Captured instruction list"));
    assert!(output.contains("σ₀ | B₀ @ 0x0000"));
    assert!(output.contains("reason=UnknownTarget"));
    assert!(output.contains("SSA unavailable"));
}

#[test]
fn invalid_nested_delegation_is_not_disassembled_as_runtime_code() {
    let owner = address(0x101);
    let delegated = address(0x200);
    let next = address(0x300);
    let mut world = World::new(Fork::Osaka, "nested delegation");
    for (source, target) in [(owner, delegated), (delegated, next)] {
        world
            .insert(
                source,
                Account::from_hex(&format!("ef0100{}", hex::encode(target)), Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    let analysis = run(world, owner, ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    assert!(output.contains("mode=InvalidDelegation"));
    assert!(output.contains("Invalid nested delegation: exceptional halt"));
    assert!(output.contains("invalid nested delegation (exceptional halt)"));
    assert!(!output.contains("Captured instruction list"));
    assert!(output.contains("| Failure |"));
}

#[test]
fn synthetic_call_continuation_is_distinct_from_a_bytecode_block() {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "synthetic continuation");
    world
        .insert(
            owner,
            Account::from_hex("5f5f5f5f5f6102005af1", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(address(0x200), Account::empty()).unwrap();
    let analysis = run(world, owner, ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    assert!(
        section(&output, "CFG", "Outcomes")
            .contains("synthetic end-of-code continuation (no instruction) | f₀ active",)
    );
    assert!(output.contains("empty code (implicit halt)"));
    assert_eq!(
        section(&output, "Execution code", "CFG")
            .matches("Captured instruction list")
            .count(),
        1,
    );
}
