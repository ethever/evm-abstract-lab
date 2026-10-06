use super::render;
use crate::{
    Fork,
    analysis::{ExecutionConfig, FrameCode, Status, WorldAnalysis, analyze_world},
    domain::Value,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256, hex, keccak256};

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn entry(owner: Address, calldata: ByteArray) -> Entry {
    Entry {
        address: owner,
        caller: address(0x900),
        value: Value::constant(U256::ZERO),
        calldata,
        is_static: false,
    }
}

fn run(world: World, owner: Address, calldata: ByteArray) -> WorldAnalysis {
    analyze_world(world, entry(owner, calldata), ExecutionConfig::default()).unwrap()
}

fn code_section(output: &str) -> &str {
    output
        .split_once("\nExecution code\n")
        .unwrap()
        .1
        .split_once("\nAnalysis\n")
        .unwrap()
        .0
}

fn captured_variant<'a>(output: &'a str, address: Address, hash: &str) -> &'a str {
    output
        .split("\n  C")
        .skip(1)
        .find(|variant| {
            variant.starts_with(|character: char| character.is_ascii_digit())
                && variant
                    .lines()
                    .next()
                    .unwrap()
                    .contains(&format!("code={address}"))
                && variant
                    .lines()
                    .next()
                    .unwrap()
                    .contains(&format!("code_hash={hash}"))
        })
        .expect("captured code variant")
}

#[test]
fn created_initcode_and_overlay_runtime_are_disassembled_from_captured_frames() {
    let creator = address(0x101);
    let deployed = creator.create(0);
    let runtime = hex::decode("602a5f5260205ff3").unwrap();
    // Initcode copies its appended runtime and returns it; factory then calls the new account.
    let mut initcode = hex::decode("61000861000d5f396100085ff3").unwrap();
    initcode.extend(&runtime);
    let mut factory =
        hex::decode("6100156100265f396100155f5ff0805f555060205f5f5f5f5f5462fffffff160015560205ff3")
            .unwrap();
    assert_eq!(factory.len(), 38);
    factory.extend(&initcode);
    let mut world = World::new(Fork::Osaka, "captured creation code");
    world
        .insert(
            creator,
            Account::from_hex(&hex::encode(&factory), Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(deployed, Account::absent()).unwrap();
    // CREATE 的保守失败分支会以零地址调用，固定快照也明确该账户为空。
    world.insert(Address::ZERO, Account::absent()).unwrap();
    let analysis = run(world, creator, ByteArray::empty());
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert_eq!(
        analysis.world().raw_account_code(deployed),
        Some(Vec::new())
    );
    assert!(analysis.states().iter().any(|state| {
        state.active().mode == FrameCode::Runtime && state.active().code_address == deployed
    }));

    let output = render(&analysis).unwrap();
    let codes = code_section(&output);
    let init = captured_variant(codes, deployed, &keccak256(&initcode).to_string());
    let deployed_runtime = captured_variant(codes, deployed, &keccak256(&runtime).to_string());
    assert!(init.contains("mode=InitCode"));
    assert!(init.contains("CODECOPY"));
    assert!(deployed_runtime.contains("mode=Runtime"));
    assert!(deployed_runtime.contains("Captured instruction list (8 bytes)"));
    assert!(deployed_runtime.contains("PUSH1          0x2a"));
    assert!(deployed_runtime.contains("executed_pcs=[0x0000, 0x0002"));
    assert_eq!(codes.matches("Captured instruction list").count(), 3);
    assert!(!codes.contains("Input code observations"));
    assert!(output.contains("Call summaries\n"));
    assert!(output.contains("Frontiers\n"));
}

#[test]
fn shared_delegate_code_is_deduplicated_but_preserves_distinct_state_owners() {
    let creator = address(0x101);
    let implementation = address(0x200);
    let mut world = World::new(Fork::Osaka, "shared code owners");
    world
        .insert(
            creator,
            Account::from_hex("5f5f5f5f6102005af4505f5f5f5f5f6102005af15000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            implementation,
            Account::from_hex("5f00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let analysis = run(world, creator, ByteArray::empty());
    assert_eq!(analysis.status(), Status::Converged);
    let output = render(&analysis).unwrap();
    let codes = code_section(&output);
    let implementation_code =
        captured_variant(codes, implementation, &keccak256([0x5f, 0x00]).to_string());
    assert!(implementation_code.contains(&format!("state owners: {creator}, {implementation}")));
    assert_eq!(codes.matches("Captured instruction list").count(), 2);
    assert!(codes.contains("frame=0 entry suspended"));
    assert!(implementation_code.contains("frame=1 entry active"));
    assert_eq!(render(&analysis).unwrap(), output);
}

#[test]
fn partial_native_frame_has_frontiers_without_invented_bytecode_or_ssa() {
    let analysis = run(
        World::new(Fork::Osaka, "unknown identity input"),
        address(4),
        ByteArray::unknown(),
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.states()[0].program().is_none());
    let output = render(&analysis).unwrap();
    let codes = code_section(&output);
    assert!(codes.contains("Native precompile"));
    assert!(codes.contains("no bytecode instruction list"));
    assert!(!codes.contains("B0 @"));
    assert!(!codes.contains("Captured instruction list"));
    assert!(output.contains("PrecompileInput"));
    assert!(output.ends_with("SSA unavailable: cross-contract frontiers remain\n"));
}

#[test]
fn empty_code_and_missing_entry_remain_distinct_without_fake_disassembly() {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "empty observed entry");
    world.insert(owner, Account::empty()).unwrap();
    let empty = run(world, owner, ByteArray::empty());
    assert_eq!(empty.status(), Status::Converged);
    let empty_output = render(&empty).unwrap();
    assert!(code_section(&empty_output).contains("Empty executable code"));
    assert!(!code_section(&empty_output).contains("B0 @"));
    assert!(!empty_output.contains("SSA unavailable"));

    let missing = run(
        World::new(Fork::Osaka, "missing entry"),
        owner,
        ByteArray::empty(),
    );
    assert_eq!(missing.status(), Status::Incomplete);
    assert!(missing.states().is_empty());
    let missing_output = render(&missing).unwrap();
    assert!(code_section(&missing_output).contains("No executable frame was captured"));
    assert!(!code_section(&missing_output).contains("B0 @"));
    assert!(missing_output.contains("MissingCode"));
    assert!(missing_output.ends_with("SSA unavailable: cross-contract frontiers remain\n"));
}

#[test]
fn initial_budget_boundary_preserves_observed_code_with_no_execution_claim() {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "initial work boundary");
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
    assert!(code_section(&output).contains("Input code observations (not execution evidence)"));
    assert!(
        code_section(&output).contains("Observed instruction list (execution not established)")
    );
    assert!(code_section(&output).contains("PUSH1          0x2a"));
    assert!(!code_section(&output).contains("Captured instruction list"));
    assert!(output.ends_with("SSA unavailable: cross-contract frontiers remain\n"));
}

#[test]
fn end_of_code_after_call_is_a_synthetic_continuation_without_an_instruction() {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "call at end of code");
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
        code_section(&output)
            .contains("synthetic end-of-code continuation (no instruction) | executed_pcs=[]")
    );
    assert_eq!(
        code_section(&output)
            .matches("Captured instruction list")
            .count(),
        1
    );
}
