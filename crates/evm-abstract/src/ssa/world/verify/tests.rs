use crate::{
    Address, Fork, U256,
    analysis::{self, ExecutionConfig},
    domain::Value,
    ssa::{self, SsaError},
    world::{Account, ByteArray, Entry, World},
};

fn example() -> analysis::WorldAnalysis {
    let mut world = World::new(Fork::Osaka, "SSA call fixture");
    let root = Address::repeat_byte(0x11);
    let child = Address::repeat_byte(0x22);
    let code = format!("60205f5f5f5f73{child:x}61fffff15060205ff3");
    world
        .insert(root, Account::from_hex(&code, Fork::Osaka).unwrap())
        .unwrap();
    world
        .insert(
            child,
            Account::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    analysis::analyze_world(
        world,
        Entry {
            address: root,
            caller: Address::repeat_byte(0x33),
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
        },
        ExecutionConfig::default(),
    )
    .unwrap()
}

#[test]
fn frame_ssa_records_deferred_results_and_effect_dependencies() {
    let analysis = example();
    let ir = ssa::build_world(&analysis).unwrap();
    assert!(
        ir.transitions()
            .iter()
            .any(|t| t.kind == analysis::MachineEdgeKind::Call && t.result.is_none())
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|t| t.kind == analysis::MachineEdgeKind::Return && t.result.is_some())
    );
    assert!(ir.effect_count() > ir.blocks().len());
}

#[test]
fn verifier_rejects_wrong_caller_arguments_results_and_effects() {
    let analysis = example();
    let valid = ssa::build_world(&analysis).unwrap();
    let return_index = valid
        .transitions
        .iter()
        .position(|t| t.kind == analysis::MachineEdgeKind::Return)
        .unwrap();
    let mut wrong = valid.clone();
    wrong.transitions[return_index].result = None;
    assert!(matches!(
        wrong.verify(&analysis),
        Err(SsaError::Invariant(_))
    ));
    let mut wrong = valid.clone();
    wrong.transitions[return_index].effect_input = valid.effect_count;
    assert!(wrong.verify(&analysis).is_err());
    let mut wrong = valid.clone();
    wrong.transitions[return_index].stacks[0].clear();
    assert!(wrong.verify(&analysis).is_err());
    let mut wrong = valid.clone();
    wrong.blocks[0].instructions[0].opcode = 0xfe;
    assert!(wrong.verify(&analysis).is_err());
    let mut wrong = valid.clone();
    let target = analysis.edges()[return_index].to;
    wrong.blocks[target].effect.inputs.clear();
    assert!(wrong.verify(&analysis).is_err());
}

#[test]
fn native_ssa_refuses_an_unresolved_world() {
    let mut world = World::new(Fork::Osaka, "missing entry");
    world.insert(Address::ZERO, Account::unknown()).unwrap();
    let analysis = analysis::analyze_world(
        world,
        Entry {
            address: Address::ZERO,
            caller: Address::ZERO,
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
        },
        ExecutionConfig::default(),
    )
    .unwrap();
    assert!(matches!(
        ssa::build_world(&analysis),
        Err(SsaError::IncompleteAnalysis)
    ));
}
