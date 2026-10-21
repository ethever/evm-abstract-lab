use crate::ssa::test_support::{
    TestAccount, TestBlobHashes, TestBytes, TestEntry, TestEnvironment, TestValue, TestWorld,
    postprocessing, product, run_program_with_environment, run_world,
};
use crate::{
    Fork,
    analysis::ExecutionConfig,
    render::world,
    ssa,
    world::{AddressInput, GasInput},
};
use alloy_primitives::{Address, B256, U256};

#[test]
fn symbolic_input_aliases_survive_all_reports_without_a_fake_address_legend() {
    let internal = Address::repeat_byte(0xee);
    let mut world = TestWorld::new(Fork::Osaka, "symbolic rendering fixture");
    world
        .insert(
            internal,
            TestAccount::from_hex("3032333400", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let mut entry = TestEntry::new(internal);
    entry.environment.to = AddressInput::unknown_to();
    let analysis = run_world(world, entry, ExecutionConfig::default()).unwrap();
    let ir = ssa::build_world(&analysis).unwrap();
    let explanation = world::explain(&analysis, &product(), &mut postprocessing()).unwrap();
    assert!(
        explanation.contains(
            "to=symbolic(To) | caller=symbolic(Caller) | origin=symbolic(Caller) (same as caller)"
        ),
        "{explanation}"
    );
    assert!(explanation.contains("value=⊤"), "{explanation}");
    assert!(
        explanation.contains("calldata: length=⊤ | content=abstract bytes"),
        "{explanation}"
    );
    for report in [
        explanation,
        world::explain_verbose(&analysis, &product(), &mut postprocessing()).unwrap(),
        world::text(&analysis, &product()),
        world::dot(&analysis, &product()),
        world::ssa(&analysis, &ir),
    ] {
        assert!(report.contains("symbolic(To)"), "{report}");
        assert!(report.contains("symbolic(Caller)"), "{report}");
        assert!(
            !report.contains(&internal.to_string()),
            "internal owner leaked as a user address:\n{report}"
        );
    }
    let json = world::json(&analysis).unwrap();
    assert_eq!(
        json["entry"]["environment"]["caller"],
        serde_json::to_value(AddressInput::unknown_caller()).unwrap()
    );
    assert_eq!(
        json["entry"]["environment"]["origin"],
        serde_json::Value::Null
    );
    assert_eq!(
        json["states"][0]["key"]["frames"][0]["address_value"],
        serde_json::to_value(AddressInput::unknown_to()).unwrap()
    );
}

#[test]
fn concrete_empty_calldata_and_independent_origin_are_explicit_with_every_override() {
    let mut input = TestEnvironment {
        origin: Some(AddressInput::unknown_origin()),
        calldata: TestBytes::empty(),
        gas_price: TestValue::constant(U256::from(2)),
        coinbase: AddressInput::Concrete(Address::repeat_byte(0x44)),
        timestamp: TestValue::constant(U256::from(3)),
        number: TestValue::constant(U256::from(4)),
        prevrandao: TestValue::constant(U256::from(5)),
        gas_limit: TestValue::constant(U256::from(6)),
        chain_id: Some(TestValue::constant(U256::from(7))),
        base_fee: TestValue::constant(U256::from(8)),
        blob_base_fee: TestValue::constant(U256::from(9)),
        gas: GasInput::UpperBound(U256::from(10)),
        blob_hashes: TestBlobHashes::exact(&[B256::repeat_byte(0x55)]),
        ..TestEnvironment::default()
    };
    input
        .block_hashes
        .insert(U256::from(1), B256::repeat_byte(0x66));
    let mut report = String::new();
    super::write(
        &mut report,
        &input,
        None,
        true,
        |address| address.to_string(),
        &product(),
    );
    assert!(
        report.contains("caller=symbolic(Caller) | origin=symbolic(Origin)"),
        "{report}"
    );
    assert!(!report.contains("same as caller"), "{report}");
    assert!(
        report.contains("calldata: length={0x0} | content=0x\n"),
        "{report}"
    );
    for field in [
        "gas price={0x2}",
        "coinbase=0x4444",
        "timestamp={0x3}",
        "block number={0x4}",
        "prevrandao={0x5}",
        "block gas limit={0x6}",
        "chain id={0x7}",
        "base fee={0x8}",
        "blob base fee={0x9}",
        "gas: upper bound=0xa",
        "configured block hashes=1",
        "blob hashes: length={0x1} | configured=1",
    ] {
        assert!(report.contains(field), "missing {field:?}:\n{report}");
    }
    assert!(
        report.contains(&B256::repeat_byte(0x55).to_string()),
        "{report}"
    );
    assert!(
        report.contains(&B256::repeat_byte(0x66).to_string()),
        "{report}"
    );
}

#[test]
fn a_concrete_caller_equal_to_the_internal_owner_keeps_its_concrete_identity() {
    let internal = Address::ZERO;
    let mut world = TestWorld::new(Fork::Osaka, "concrete caller at internal key");
    world
        .insert(internal, TestAccount::from_hex("00", Fork::Osaka).unwrap())
        .unwrap();
    let mut entry = TestEntry::new(internal);
    entry.environment.to = AddressInput::unknown_to();
    entry.environment.caller = AddressInput::Concrete(internal);
    let analysis = run_world(world, entry, ExecutionConfig::default()).unwrap();
    let report = world::explain(&analysis, &product(), &mut postprocessing()).unwrap();
    assert!(
        report.contains("to=symbolic(To) | caller=A₀ | origin=A₀ (same as caller)"),
        "{report}"
    );
    assert!(report.contains(&format!("A₀ = {internal}")), "{report}");
    assert!(report.contains("code=symbolic(To)"), "{report}");
}

#[test]
fn single_program_cfg_ssa_and_dot_keep_the_execution_input_scope_visible() {
    let program = crate::bytecode::Program::from_hex_with_fork("3032333400", Fork::Osaka).unwrap();
    let environment = TestEnvironment::default();
    let analysis =
        run_program_with_environment(program, crate::analysis::Config::default(), environment)
            .unwrap();
    let ir = ssa::build(&analysis).unwrap();
    for report in [
        crate::render::cfg(&analysis, &product()),
        crate::render::ssa(&analysis, &ir, &product()),
        crate::render::dot(&analysis, &product()),
    ] {
        assert!(report.contains("to=symbolic(To) | caller=symbolic(Caller) | origin=symbolic(Caller) (same as caller)"), "{report}");
        assert!(report.contains("calldata: length=⊤"), "{report}");
    }
}
