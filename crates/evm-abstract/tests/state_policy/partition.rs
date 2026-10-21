use super::{
    oracle,
    support::{
        ROOT, address, alternate_run, branches, complete, config, default_run, entry, input, run,
        starts_at, word, world,
    },
};
use alloy_primitives::{B256, U256, keccak256};
use evm_abstract::{
    analysis::{
        CorrelatedJoin, EnginePolicies, FrameKey, MachineError, MachineKey, MachinePartition,
        MachinePayload, MergeResumeStatus, NoAcceleration, Partitioned, PartitionedRelative,
        PolicyContext, PolicyKey, PolicySite, ProjectedSummaries, RecentDistinctJumps,
        RelativeState, SemanticPolicies, StructuralPartition,
    },
    bytecode::Program,
    domain::Domain,
    ssa,
    world::{ByteArray, Code},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum OperandClass {
    Zero,
    Nonzero,
    MaybeZero,
}

/// The special control point has a policy-owned identity. There is no mandatory
/// original MachineKey outside it: histories are erased before grouping.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum OperandKey {
    AtOperand {
        frame: Box<FrameKey>,
        code_identity: B256,
        operand: OperandClass,
    },
    Elsewhere(MachineKey),
}

impl PolicyKey for OperandKey {
    fn key_work(&self) -> usize {
        match self {
            Self::AtOperand { frame, .. } => frame.key_work().saturating_add(10),
            Self::Elsewhere(key) => key.key_work().saturating_add(1),
        }
    }
}

/// Intentionally non-Clone. The program and chosen PC belong to the caller.
struct OperandPartition<'a> {
    program: &'a Program,
    code_hash: B256,
    pc: usize,
}

impl MachinePartition<Domain, ()> for OperandPartition<'_> {
    type Key = OperandKey;

    fn prepare(
        &self,
        input: &MachinePayload,
        site: PolicySite,
        class: (),
        cx: &mut PolicyContext<'_, Domain>,
    ) -> Result<Partitioned<Self::Key, evm_abstract::domain::NumericValue>, MachineError> {
        cx.reserve(input.policy_work_size().saturating_add(1))?;
        let frame = &input.active().key;
        let selected = site == PolicySite::World
            && input.call_stack.depth() == 1
            && frame.code_hash == self.code_hash
            && self
                .program
                .blocks()
                .get(frame.basic_block_index)
                .is_some_and(|block| block.start_pc == self.pc);
        if !selected {
            let prepared = StructuralPartition.prepare(input, site, class, cx)?;
            return Ok(Partitioned {
                key: OperandKey::Elsewhere(prepared.key.0),
                payload: prepared.payload,
            });
        }
        let operand = input.active().stack.last().ok_or(MachineError::Shape)?;
        let operand = if operand.singleton() == Some(U256::ZERO) {
            OperandClass::Zero
        } else if !operand.contains(U256::ZERO) {
            OperandClass::Nonzero
        } else {
            OperandClass::MaybeZero
        };
        let payload = input.forget_jump_history(cx)?;
        let shape = payload.key();
        let key = OperandKey::AtOperand {
            frame: Box::new(shape.frames[0].clone()),
            code_identity: shape.code_identity,
            operand,
        };
        cx.reserve(0)?;
        Ok(Partitioned { key, payload })
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<evm_abstract::domain::NumericValue>,
        site: PolicySite,
        class: (),
        cx: &mut PolicyContext<'_, Domain>,
    ) -> Result<PartitionedRelative<Self::Key, evm_abstract::domain::NumericValue>, MachineError>
    {
        let prepared = StructuralPartition.prepare_relative(input, site, class, cx)?;
        Ok(PartitionedRelative {
            key: OperandKey::Elsewhere(prepared.key.0),
            payload: prepared.payload,
        })
    }
}

#[test]
fn borrowed_operand_key_groups_different_histories_without_losing_concrete_tuples() {
    let fixture = branches(false);
    let world = world(&fixture);
    let entry = entry(ByteArray::unknown());
    let pc = fixture.labels["join"];
    let Code::Runtime(program) = &world.account(address(ROOT)).unwrap().code else {
        panic!("captured program");
    };
    let policies = EnginePolicies::new(RecentDistinctJumps, MergeResumeStatus).with_state(
        SemanticPolicies::new(
            OperandPartition {
                program,
                code_hash: keccak256(program.bytes()),
                pc,
            },
            CorrelatedJoin,
            NoAcceleration,
            ProjectedSummaries,
        ),
    );
    let structural = default_run(&world, &entry, config());
    let forgotten = alternate_run(&world, &entry, config());
    let predicate = run(&world, &entry, config(), &policies);
    ssa::build_world(&predicate)
        .unwrap()
        .verify(&predicate)
        .unwrap();
    for graph in [&structural, &forgotten, &predicate] {
        complete(graph);
        for (n, expected) in [(0, 0), (1, 11), (2, 22)] {
            let witness = oracle::execute(&world, &super::support::entry(input(n)));
            assert_eq!(witness.output_word(0), U256::from(expected));
            assert_eq!(witness.output_word(32), U256::from(expected));
            assert_eq!(
                witness.storage[&(address(ROOT), U256::ZERO)],
                U256::from(expected)
            );
            witness.covers(graph);
        }
    }
    let path = |n| {
        structural
            .states()
            .iter()
            .find(|state| {
                starts_at(state, pc)
                    && state.entry.active().stack[0].singleton() == Some(U256::from(n))
            })
            .unwrap()
    };
    assert_ne!(
        path(11).active().jump_history,
        path(22).active().jump_history
    );
    assert!(!path(11).active().jump_history.is_empty());

    let at_join: Vec<_> = predicate
        .states()
        .iter()
        .filter(|state| starts_at(state, pc))
        .collect();
    let zero = at_join
        .iter()
        .find(|state| state.entry.active().stack[0].singleton() == Some(U256::ZERO))
        .expect("the input-operand predicate keeps zero separate");
    assert_eq!(
        zero.entry
            .store
            .read(address(ROOT), &word(0), Domain::default())
            .singleton(),
        Some(U256::ZERO)
    );
    let nonzero: Vec<_> = at_join
        .iter()
        .filter(|state| !state.entry.active().stack[0].contains(U256::ZERO))
        .collect();
    assert_eq!(
        nonzero.len(),
        1,
        "both nonzero histories must enter one policy-owned identity"
    );
    let merged = nonzero[0];
    assert!(merged.active().jump_history.is_empty());
    for n in [11, 22] {
        assert!(merged.entry.active().stack[0].contains(U256::from(n)));
        assert!(
            merged
                .entry
                .active()
                .memory
                .read_word(&word(0), Domain::default())
                .contains(U256::from(n))
        );
        assert!(
            merged
                .entry
                .store
                .read(address(ROOT), &word(0), Domain::default())
                .contains(U256::from(n))
        );
    }
    assert!(
        forgotten
            .states()
            .iter()
            .filter(|state| starts_at(state, pc))
            .any(|state| state.active().jump_history.is_empty()
                && [0, 11, 22]
                    .into_iter()
                    .all(|n| state.entry.active().stack[0].contains(U256::from(n))))
    );
}
