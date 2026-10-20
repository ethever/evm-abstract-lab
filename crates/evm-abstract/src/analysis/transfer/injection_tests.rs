//! Executable carrier substitution through transfer, checkpoints and completion.

use super::*;
use crate::{
    Fork,
    analysis::engine::policy::{ContextInsensitive, RecentDistinctJumps},
    domain::{
        DomainSpec, NumericReduction, ValueAlgebra, WordObservation, facts::FactError,
        known_bits::KnownBits,
    },
    world::EvmAccount,
};
use serde::Serialize;
use std::{cell::Cell, collections::BTreeSet, fmt};

/// This carrier has no finite set or fixed product. Exact words are known bits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Bits(KnownBits);

impl fmt::Display for Bits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl WordElement for Bits {
    fn top() -> Self {
        Self(KnownBits::top())
    }
    fn constant(word: U256) -> Self {
        Self(KnownBits::exact(word))
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self(*observation.known_bits())
    }
    fn observation(&self) -> WordObservation {
        WordObservation::bits(self.0)
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        self.observation()
            .meet(observation)
            .map(|facts| Self::from_observation(&facts))
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        None
    }
    fn known_bits(&self) -> &KnownBits {
        &self.0
    }
    fn singleton(&self) -> Option<U256> {
        self.0.singleton()
    }
    fn contains(&self, word: U256) -> bool {
        self.0.contains(word)
    }
    fn is_top(&self) -> bool {
        self.0 == KnownBits::top()
    }
    fn work_size(&self) -> usize {
        // Includes the fixed-size observation export, not just the eight limbs.
        64
    }
}

/// Deliberately non-Copy and non-Clone: transfer must borrow this exact policy.
#[derive(Debug)]
struct BitsAlgebra {
    spec: DomainSpec,
    operations: Cell<usize>,
}

impl EvmNumericAlgebra for BitsAlgebra {
    type Element = Bits;
    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, value: &Bits) -> Bits {
        value.clone()
    }
    fn join_numeric(&self, left: &Bits, right: &Bits) -> Bits {
        Bits(left.0.join(&right.0))
    }
    fn widen_numeric(&self, old: &Bits, next: &Bits) -> Bits {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Bits, next: &Bits) -> Bits {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Bits) -> NumericReduction<Bits> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[Bits]) -> NumericReduction<Bits> {
        self.operations.set(self.operations.get() + 1);
        let args = args.iter().map(|value| value.0).collect::<Vec<_>>();
        NumericReduction::stable(Bits(KnownBits::transfer(op, &args)))
    }
    fn operation_work<'a>(&self, _args: impl Iterator<Item = &'a Bits>) -> usize
    where
        Bits: 'a,
    {
        64 * 1024
    }
    fn projection_work(&self, _value: &Bits) -> usize {
        8
    }
    fn refinement_work(&self, _value: &Bits, facts: usize) -> usize {
        facts.saturating_add(1).saturating_mul(256)
    }
}

fn setup() -> (ExecutionConfig, ValueAlgebra<BitsAlgebra>) {
    let mut config = crate::analysis::machine::fixtures::config();
    config.analysis.relations.enabled = false;
    config.max_work = 1_000_000_000;
    let algebra = ValueAlgebra::new(BitsAlgebra {
        spec: config.domain().unwrap().spec(),
        operations: Cell::new(0),
    });
    (config, algebra)
}

fn owner() -> Address {
    Address::from_word(U256::from(0x101).into())
}
fn target() -> Address {
    Address::from_word(U256::from(0x202).into())
}

fn account(code: &str) -> EvmAccount<Bits> {
    let mut account = EvmAccount::from_hex(code, Fork::Osaka).unwrap();
    account.balance = EvmValue::constant(U256::from(1_000_000));
    account.nonce = EvmValue::constant(U256::ZERO);
    account
}

fn run<C: ContextPolicy>(
    world: &EvmWorld<Bits>,
    entry: &EvmEntry<Bits>,
    payload: EvmMachinePayload<Bits>,
    config: &ExecutionConfig,
    algebra: &ValueAlgebra<BitsAlgebra>,
    policy: &C,
) -> EvmExecution<Bits> {
    execute(
        world,
        entry,
        payload,
        config,
        algebra.borrowed(),
        policy,
        &mut WorkBudget::new(config.max_work),
        StorageReadPolicy::Abstract,
    )
}

#[test]
fn independent_carrier_survives_delegatecall_revert_memory_and_saved_store() {
    let (config, algebra) = setup();
    let mut world = EvmWorld::new(Fork::Osaka, "injected delegate rollback");
    world
        .insert(
            owner(),
            account("60075f55602a5f525f5f5f5f61020262fffffff4505f545f5100"),
        )
        .unwrap();
    world
        .insert(target(), account("60095f5560635f5260205ffd"))
        .unwrap();
    let entry = EvmEntry::new(owner());
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let entered = run(
        &world,
        &entry,
        input,
        &config,
        &algebra,
        &RecentDistinctJumps,
    );
    assert!(entered.frontiers.is_empty());
    let child = entered
        .successors
        .into_iter()
        .find(|next| next.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    let key = EvmValue::<Bits>::constant(U256::ZERO);
    let saved = child
        .active()
        .saved_store
        .state()
        .read_with(owner(), &key, algebra.borrowed());
    let _: &Bits = saved.numeric();
    assert_eq!(saved.singleton(), Some(U256::from(7)));
    assert_eq!(child.active().key.address, owner());
    assert_eq!(child.active().key.code_address, target());
    let returned = run(
        &world,
        &entry,
        child,
        &config,
        &algebra,
        &RecentDistinctJumps,
    );
    assert!(returned.frontiers.is_empty());
    let resumed = returned
        .successors
        .into_iter()
        .find(|next| next.kind == MachineEdgeKind::Revert)
        .unwrap()
        .payload;
    assert_eq!(
        resumed.active().stack.last().unwrap().singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        resumed
            .store
            .read_with(owner(), &key, algebra.borrowed())
            .singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        resumed
            .active()
            .returndata
            .read_word_checked_with(&key, algebra.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::from(99))
    );
    let completed = run(
        &world,
        &entry,
        resumed,
        &config,
        &algebra,
        &RecentDistinctJumps,
    );
    assert!(completed.frontiers.is_empty());
    assert_eq!(
        completed
            .payload
            .active()
            .stack
            .iter()
            .map(EvmValue::singleton)
            .collect::<Vec<_>>(),
        vec![Some(U256::from(7)), Some(U256::from(42))]
    );
    assert!(
        completed
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    assert!(algebra.numeric_algebra().operations.get() > 0);
}

#[test]
fn injected_context_controls_normal_and_procedure_jump_execution() {
    let (mut config, algebra) = setup();
    config.analysis.context_depth = 4;
    let mut world = EvmWorld::new(Fork::Osaka, "injected jump policy");
    world.insert(owner(), account("6003565b00")).unwrap();
    let entry = EvmEntry::new(owner());
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    for procedure in [false, true] {
        let execute = |policy: &RecentDistinctJumps| {
            execute_inner(
                &world,
                &entry,
                input.clone(),
                &config,
                algebra.borrowed(),
                policy,
                &mut WorkBudget::new(config.max_work),
                StorageReadPolicy::Abstract,
                procedure,
            )
        };
        let retained = execute(&RecentDistinctJumps);
        assert!(retained.frontiers.is_empty());
        assert_eq!(
            retained.successors[0].payload.active().key.jump_history,
            vec![0]
        );
        let erased = execute_inner(
            &world,
            &entry,
            input.clone(),
            &config,
            algebra.borrowed(),
            &ContextInsensitive,
            &mut WorkBudget::new(config.max_work),
            StorageReadPolicy::Abstract,
            procedure,
        );
        assert!(erased.frontiers.is_empty());
        assert!(
            erased.successors[0]
                .payload
                .active()
                .key
                .jump_history
                .is_empty()
        );
        assert_eq!(
            erased.successors[0].payload.active().key.basic_block_index,
            retained.successors[0]
                .payload
                .active()
                .key
                .basic_block_index
        );
    }
}

#[test]
fn singleton_without_finite_membership_enters_create_and_create2() {
    let (config, algebra) = setup();
    for create2 in [false, true] {
        let mut world = EvmWorld::new(Fork::Osaka, "injected creation literals");
        let destination = if create2 {
            owner().create2_from_code(U256::from(5).to_be_bytes::<32>(), [0u8])
        } else {
            owner().create(0)
        };
        world
            .insert(
                owner(),
                account(if create2 {
                    "5f5f53600560015f5ff500"
                } else {
                    "5f5f5360015f5ff000"
                }),
            )
            .unwrap();
        world
            .insert(destination, EvmAccount::<Bits>::empty())
            .unwrap();
        let entry = EvmEntry::new(owner());
        let input = initial(&world, &entry, algebra.borrowed()).unwrap();
        let entered = run(
            &world,
            &entry,
            input,
            &config,
            &algebra,
            &RecentDistinctJumps,
        );
        assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
        let child = entered
            .successors
            .into_iter()
            .find(|next| next.kind == MachineEdgeKind::Call)
            .unwrap()
            .payload;
        assert_eq!(child.active().code, FrameCode::InitCode);
        assert_eq!(child.active().key.address, destination);
        let returned = run(
            &world,
            &entry,
            child,
            &config,
            &algebra,
            &RecentDistinctJumps,
        );
        assert!(returned.frontiers.is_empty());
        let resumed = returned
            .successors
            .iter()
            .find(|next| next.kind == MachineEdgeKind::Return)
            .unwrap();
        assert_eq!(
            resumed.payload.active().stack.last().unwrap().singleton(),
            Some(U256::from_be_slice(destination.as_slice()))
        );
    }
}

#[test]
fn native_identity_keeps_the_injected_byte_carrier() {
    let (config, algebra) = setup();
    let world = EvmWorld::new(Fork::Osaka, "injected native identity");
    let native = Address::from_word(U256::from(4).into());
    let entry = EvmEntry::concrete(
        native,
        owner(),
        EvmValue::<Bits>::constant(U256::ZERO),
        EvmByteArray::exact(&[0x12, 0x34, 0x56]),
    );
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let returned = run(
        &world,
        &entry,
        input,
        &config,
        &algebra,
        &RecentDistinctJumps,
    );
    assert!(returned.frontiers.is_empty());
    let data = &returned
        .outcomes
        .iter()
        .find(|outcome| outcome.kind == OutcomeKind::Return)
        .unwrap()
        .data;
    let _: &Bits = data.len().numeric();
    assert_eq!(data.len().singleton(), Some(U256::from(3)));
    for (offset, expected) in [0x12u8, 0x34, 0x56].into_iter().enumerate() {
        assert_eq!(
            data.byte_at_with(offset, algebra.borrowed()).singleton(),
            Some(U256::from(expected))
        );
    }
}
