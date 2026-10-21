//! 组合域必须改变真实分析的可观察精度，同时覆盖全部具体可能值。
//! 常量容量、临时规约边界与全局执行完成状态分别核对。
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;
use evm_abstract::domain::{EvmNumericAlgebra, FiniteConstantSet};

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::{NumericValue, ProductAlgebra};
use evm_abstract::{
    Address, Fork, U256,
    analysis::{EdgeKind, EvmAnalysis, ExecutionConfig, OutcomeKind, Status},
    bytecode::Program,
    domain::{
        DomainSpec, EvmValue, ReductionStatus, ValueAlgebra,
        facts::{BitConstraints, BitIndex, FactError, UnaryPredicate, WordBounds},
        provenance::Origin,
        symbolic::{ExprKind, SlotStateKind},
    },
    ssa,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmStore, EvmWorld},
};
use proptest::{arbitrary::any, collection::vec, prop_assert, prop_assert_eq, proptest};
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

fn product(capacity: usize) -> ValueAlgebra<ProductAlgebra> {
    native::product_with(DomainSpec::new(
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
    ))
}

fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(value))
}

fn bounds(lo: U256, hi: U256) -> UnaryPredicate {
    UnaryPredicate::UnsignedBounds(WordBounds::new(lo, hi).unwrap())
}

fn set<D: EvmNumericAlgebra>(domain: &ValueAlgebra<D>, values: &[u16]) -> EvmValue<D::Element> {
    values[1..].iter().fold(
        EvmValue::<D::Element>::constant(U256::from(values[0])),
        |old, value| domain.join(&old, &EvmValue::<D::Element>::constant(U256::from(*value))),
    )
}

fn numeric_case<D: EvmNumericAlgebra>(
    values: &ValueAlgebra<D>,
    code: &str,
) -> EvmAnalysis<D::Element> {
    let input =
        AnalysisInput::from_program(Program::from_hex(code).unwrap(), Default::default()).unwrap();
    let graph = native::run(
        values,
        input,
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap()
}

fn comparison_spec(capacity: usize) -> DomainSpec {
    native::spec(capacity, 4, 1024).with_relations(
        evm_abstract::domain::relational::RelationLimits {
            enabled: false,
            ..Default::default()
        },
    )
}

fn odd_byte(domain: &ValueAlgebra<ProductAlgebra>) -> EvmValue<NumericValue> {
    domain
        .from_facts(&[
            bounds(U256::ZERO, U256::from(255)),
            UnaryPredicate::BitSet(BitIndex::new(0).unwrap()),
        ])
        .unwrap()
}

#[test]
fn finite_capacity_loss_preserves_independent_numeric_components() {
    let domain = product(1);
    let value = set(&domain, &[2, 5, 8]);
    assert!(value.constants().is_none());
    assert_eq!(value.finite_constants().to_string(), "⊤");
    assert!(
        value
            .to_string()
            .contains(&format!("bits=0x{}*", "0".repeat(63))),
        "a Top constant component must preserve the other numeric constraints"
    );
    assert_eq!(
        value.interval().unsigned_bounds(),
        (U256::from(2), U256::from(8))
    );
    assert_eq!(value.known_bits().zero(), !U256::from(15));
    assert_eq!(
        value.congruence().modulus_residue(),
        Some((U256::from(3), U256::from(2)))
    );
    for member in [2_u64, 5, 8] {
        assert!(value.contains(U256::from(member)));
    }
    for outside in [0_u64, 1, 3, 4, 6, 7, 9, 16] {
        assert!(!value.contains(U256::from(outside)));
    }
    let baseline = set(&native::finite_with(native::spec(1, 4, 256)), &[2, 5, 8]);
    assert_eq!(
        baseline.numeric(),
        &evm_abstract::domain::FiniteConstantSet::top()
    );
    assert_eq!(baseline.to_string(), "⊤");
    assert!(baseline.contains(U256::MAX));
}

#[test]
fn semantic_facts_combine_crt_masks_and_ranges_into_an_exact_value() {
    let domain = product(2);
    // x=9 mod 12; 区间留下 21 与 33，bit 4 clear 进一步排除 21。
    let value = domain
        .from_facts(&[
            bounds(U256::from(10), U256::from(40)),
            UnaryPredicate::congruent(U256::from(4), U256::from(1)).unwrap(),
            UnaryPredicate::congruent(U256::from(6), U256::from(3)).unwrap(),
            UnaryPredicate::BitClear(BitIndex::new(4).unwrap()),
        ])
        .unwrap();
    assert_eq!(value.singleton(), Some(U256::from(33)));
    assert_eq!(value.constants().unwrap().len(), 1);
    assert_eq!(value.known_bits().singleton(), Some(U256::from(33)));
    assert_eq!(value.interval().singleton(), Some(U256::from(33)));
    assert_eq!(value.congruence().singleton(), Some(U256::from(33)));
    assert!(!value.contains(U256::from(21)));
    let stable = domain.reduce(&value);
    assert_eq!(stable.status, ReductionStatus::Stable);
    assert_eq!(stable.value, value);
}

#[test]
fn complete_finite_inference_never_truncates_a_large_candidate_cover() {
    let domain = product(2);
    let value = domain
        .from_facts(&[bounds(U256::ZERO, U256::from(255))])
        .unwrap();
    assert!(value.constants().is_none());
    assert!(value.contains(U256::ZERO));
    assert!(value.contains(U256::from(127)));
    assert!(value.contains(U256::from(255)));
    assert!(!value.contains(U256::from(256)));
    assert_eq!(domain.reduce(&value).status, ReductionStatus::Stable);
}

#[test]
fn boolean_clz_and_byte_bounds_survive_a_one_constant_capacity() {
    let domain = product(1);
    let boolean = domain.apply(
        opcode::EQ,
        &[
            EvmValue::<NumericValue>::top(),
            EvmValue::<NumericValue>::top(),
        ],
    );
    assert!(boolean.constants().is_none());
    assert!(boolean.contains(U256::ZERO));
    assert!(boolean.contains(U256::from(1)));
    assert!(!boolean.contains(U256::from(2)));
    let clz = domain.apply(opcode::CLZ, &[EvmValue::<NumericValue>::top()]);
    assert!(clz.constants().is_none());
    assert_eq!(
        clz.interval().unsigned_bounds(),
        (U256::ZERO, U256::from(256))
    );
    for possible in 0_u64..=256 {
        assert!(clz.contains(U256::from(possible)));
    }
    assert!(!clz.contains(U256::from(257)));
    let byte = domain.apply(
        opcode::BYTE,
        &[
            EvmValue::<NumericValue>::top(),
            EvmValue::<NumericValue>::top(),
        ],
    );
    assert_eq!(byte.known_bits().zero(), !U256::from(255));
    for possible in 0_u64..=255 {
        assert!(byte.contains(U256::from(possible)));
    }
    assert!(!byte.contains(U256::from(256)));
    let baseline = &native::finite_with(native::spec(1, 4, 256));
    assert_eq!(
        baseline.apply(
            opcode::EQ,
            &[
                EvmValue::<FiniteConstantSet>::top(),
                EvmValue::<FiniteConstantSet>::top()
            ]
        ),
        EvmValue::<FiniteConstantSet>::top()
    );
}

#[test]
fn mask_and_interval_exchange_uses_all_256_bits_at_signed_boundary() {
    let domain = product(1);
    let sign: U256 = U256::from(1) << 255;
    let value = domain
        .from_facts(&[
            bounds(sign - U256::from(3), sign + U256::from(3)),
            UnaryPredicate::KnownBits(BitConstraints::new(U256::from(3), sign).unwrap()),
        ])
        .unwrap();
    assert_eq!(value.singleton(), Some(sign));
    assert!(value.contains(sign));
    assert!(!value.contains(sign - U256::from(1)));
    assert!(!value.contains(sign + U256::from(1)));
    let signed = domain
        .from_facts(&[
            UnaryPredicate::SignedBounds(WordBounds::new(U256::ZERO, U256::from(3)).unwrap()),
            UnaryPredicate::KnownBits(BitConstraints::new(U256::from(3), U256::ZERO).unwrap()),
        ])
        .unwrap();
    assert_eq!(signed.singleton(), Some(sign));
}

#[test]
fn conflicting_mask_range_and_congruence_inputs_are_typed_errors() {
    let domain = product(1);
    assert!(matches!(
        domain.from_facts(&[
            UnaryPredicate::congruent(U256::from(3), U256::ZERO).unwrap(),
            UnaryPredicate::congruent(U256::from(3), U256::from(1)).unwrap(),
        ]),
        Err(FactError::Contradiction { .. })
    ));
    // 不能依赖区间可枚举：容量只有 1，bit 3 必为一已经证明空交。
    assert!(
        domain
            .from_facts(&[
                bounds(U256::ZERO, U256::from(2)),
                UnaryPredicate::BitSet(BitIndex::new(3).unwrap()),
            ])
            .is_err()
    );
    let boundary: U256 = U256::from(1) << 160;
    assert!(
        domain
            .from_facts(&[
                bounds(boundary - U256::from(3), boundary + U256::from(3)),
                UnaryPredicate::multiple_of(U256::from(4)).unwrap(),
                UnaryPredicate::IsAddress,
            ])
            .is_err()
    );
}

#[test]
fn address_range_and_code_address_role_are_different_semantic_guarantees() {
    let domain = product(1);
    let address = domain.from_facts(&[UnaryPredicate::IsAddress]).unwrap();
    assert!(address.contains((U256::from(1) << 160) - U256::from(1)));
    assert!(!address.contains(U256::from(1) << 160));
    assert!(!address.contains(U256::MAX));
    let code_role = domain.from_facts(&[UnaryPredicate::IsCodeAddress]).unwrap();
    assert!(code_role.provenance().is_code_address());
    assert!(code_role.contains(U256::MAX));
}

#[test]
fn odd_modulus_facts_do_not_cross_word_wrap_unchanged() {
    let domain = product(1);
    let input = domain
        .from_facts(&[
            bounds(U256::MAX - U256::from(4), U256::MAX - U256::from(1)),
            UnaryPredicate::congruent(U256::from(3), U256::from(2)).unwrap(),
        ])
        .unwrap();
    assert!(input.constants().is_none());
    let sum = domain.apply(opcode::ADD, &[input.clone(), word(2)]);
    assert!(sum.contains(U256::ZERO));
    assert!(sum.contains(U256::MAX - U256::from(2)));
    assert!(sum.congruence().is_top());
    let difference = domain.apply(
        opcode::SUB,
        &[input, EvmValue::<NumericValue>::constant(U256::MAX)],
    );
    assert!(difference.contains(U256::MAX));
    assert!(difference.contains(U256::MAX - U256::from(3)));
}

#[test]
fn precision_limits_keep_sound_partial_results_and_report_the_boundary() {
    let domain = native::product_with(DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
    ));
    let input = EvmValue::<NumericValue>::top();
    let partial = domain.apply_detailed(opcode::MUL, &[input.clone(), word(0)]);
    assert_eq!(partial.status, ReductionStatus::RoundLimit);
    assert_eq!(partial.rounds, 1);
    assert_eq!(partial.value.singleton(), Some(U256::ZERO));
    assert!(partial.strengthened > 0);
    assert_eq!(input, EvmValue::<NumericValue>::top());
    assert_eq!(
        domain.reduce(&partial.value).status,
        ReductionStatus::Stable
    );
    let tiny = native::product_with(DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    ));
    let stopped = tiny.apply_detailed(opcode::MUL, &[EvmValue::<NumericValue>::top(), word(0)]);
    assert_eq!(stopped.status, ReductionStatus::FactLimit);
    assert_eq!(stopped.value.singleton(), Some(U256::ZERO));
}

#[test]
fn shared_value_identity_proves_copies_in_each_numeric_algebra() {
    check_copy_identity(&native::product_with(comparison_spec(1)));
    check_copy_identity(&native::finite_with(comparison_spec(1)));
}
fn check_copy_identity<D: EvmNumericAlgebra>(values: &ValueAlgebra<D>) {
    // The arithmetic result has a temporary definition; immutable calldata
    // symbols themselves now retain equality in both numeric profiles.
    for code in ["5f35600101801800", "5f3560010180901800"] {
        let graph = numeric_case(values, code);
        assert_eq!(graph.status(), Status::Converged);
        assert_eq!(
            graph.states()[0].exit_stack[0].singleton(),
            Some(U256::ZERO)
        );
        let origins = graph.states()[0].exit_stack[0]
            .provenance()
            .origins()
            .sources()
            .unwrap();
        assert!(origins.contains(&Origin::Calldata));
        assert!(origins.contains(&Origin::Arithmetic));
        ssa::build(&graph).unwrap().verify(&graph).unwrap();
    }
}

#[test]
fn separate_input_reads_do_not_gain_identity_from_same_source_label() {
    let graph = numeric_case(&native::product_with(comparison_spec(1)), "5f356020351800");
    assert_eq!(graph.status(), Status::Converged);
    let result = &graph.states()[0].exit_stack[0];
    assert!(result.contains(U256::ZERO));
    assert!(result.contains(U256::from(1)));
    assert!(result.contains(U256::MAX));
    assert_eq!(result.singleton(), None);
    ssa::build(&graph).unwrap().verify(&graph).unwrap();
}

#[test]
fn numeric_facts_prune_the_impossible_jumpi_fallthrough() {
    // 条件 (CALLDATALOAD(0) & 0xfe) | 1 永远非零，但有 128 个值。
    let code = "5f3560fe16600117600c57005b600200";
    let graph = numeric_case(&native::product_with(comparison_spec(1)), code);
    assert_eq!(graph.status(), Status::Converged);
    assert_eq!(graph.edges().len(), 1);
    assert_eq!(
        graph.edges()[0].kind,
        evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)
    );
    assert_eq!(
        graph.program().blocks()[graph.states()[graph.edges()[0].to].key.basic_block_index]
            .start_pc,
        12
    );
    ssa::build(&graph).unwrap().verify(&graph).unwrap();
    let baseline = numeric_case(&native::finite_with(comparison_spec(1)), code);
    assert!(baseline.edges().iter().any(|edge| edge.kind
        == evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse)));
}

#[test]
fn open_jump_membership_discards_only_proven_impossible_destinations() {
    // (unknown & 2) + 9 只能跳到 9 或 11；pc 13 也是合法 JUMPDEST。
    let graph = numeric_case(
        &native::product_with(comparison_spec(1)),
        "5f35600216600901565b005b005b00",
    );
    let targets = graph
        .edges()
        .iter()
        .map(|edge| {
            graph.program().blocks()[graph.states()[edge.to].key.basic_block_index].start_pc
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(targets, std::collections::BTreeSet::from([9, 11]));
}

#[test]
fn memory_and_storage_execution_preserve_nonfinite_byte_constraints() {
    for code in ["5f3560fe166001175f525f5100", "5f3560fe166001175f555f5400"] {
        let graph = numeric_case(&native::product_with(comparison_spec(1)), code);
        assert_eq!(graph.status(), Status::Converged);
        let result = &graph.states()[0].exit_stack[0];
        assert!(result.constants().is_none());
        for possible in (1_u64..=255).step_by(2) {
            assert!(result.contains(U256::from(possible)));
        }
        assert!(!result.contains(U256::ZERO));
        assert!(!result.contains(U256::from(2)));
        assert!(!result.contains(U256::from(256)));
        ssa::build(&graph).unwrap().verify(&graph).unwrap();
    }
}

#[test]
fn sparse_bytes_store_and_rollback_preserve_the_whole_product() {
    let domain = product(1);
    let initial = odd_byte(&domain);
    assert!(initial.constants().is_none());
    let mut memory = EvmByteArray::<NumericValue>::memory();
    memory
        .write_word(&word(0), &initial, domain.borrowed())
        .unwrap();
    let loaded = memory.read_word(&word(0), domain.borrowed());
    for possible in (1_u64..=255).step_by(2) {
        assert!(loaded.contains(U256::from(possible)));
    }
    assert!(!loaded.contains(U256::ZERO));
    assert!(!loaded.contains(U256::from(256)));
    let address = Address::repeat_byte(0x44);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "product checkpoint");
    world
        .insert(address, EvmAccount::<NumericValue>::empty())
        .unwrap();
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    store.write(address, &word(0), &initial, domain.borrowed());
    store.write_transient(address, &word(1), &initial, domain.borrowed());
    let persistent = store.read(address, &word(0), domain.borrowed());
    let transient = store.read_transient(address, &word(1), domain.borrowed());
    // Numeric-only bounded operands acquire a constrained observed cell on write.
    // Its binding must preserve the entire product, independently of read identity.
    for observed in [&persistent, &transient] {
        assert_eq!(observed.numeric(), initial.numeric());
        assert_eq!(observed.provenance(), initial.provenance());
        let ExprKind::SlotRead { state, key } = observed.expression().unwrap().kind() else {
            panic!("bounded numeric operand must retain its observed-cell read");
        };
        assert_eq!(key.as_constant(), Some(U256::ZERO));
        let SlotStateKind::Input { source, bindings } = state.kind() else {
            panic!("bounded operand must be tied to a fixed observation");
        };
        assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].index, U256::ZERO);
        assert_eq!(&bindings[0].numeric, initial.numeric());
        assert!(matches!(bindings[0].value.kind(), ExprKind::Fresh(_)));
    }
    assert_ne!(persistent.expression(), transient.expression());
    assert_eq!(store.read(address, &word(0), domain.borrowed()), persistent);
    assert_eq!(
        store.read_transient(address, &word(1), domain.borrowed()),
        transient
    );
    let saved = store.snapshot();
    let original = store.clone();
    store.write(address, &word(0), &word(2), domain.borrowed());
    store.write_transient(
        address,
        &word(1),
        &EvmValue::<NumericValue>::top(),
        domain.borrowed(),
    );
    assert_eq!(store.read(address, &word(0), domain.borrowed()), word(2));
    assert_eq!(saved.state(), &original);
    assert_eq!(
        saved.state().read(address, &word(0), domain.borrowed()),
        persistent
    );
    assert_eq!(
        saved
            .state()
            .read_transient(address, &word(1), domain.borrowed()),
        transient
    );
    store.restore(saved);
    assert_eq!(store, original);
    assert_eq!(store.read(address, &word(0), domain.borrowed()), persistent);
    assert_eq!(
        store.read_transient(address, &word(1), domain.borrowed()),
        transient
    );
    assert_eq!(persistent.numeric(), initial.numeric());
    assert_eq!(persistent.provenance(), initial.provenance());
    assert_eq!(transient.numeric(), initial.numeric());
    assert_eq!(transient.provenance(), initial.provenance());
}

#[test]
fn root_revert_restores_nonfinite_initial_storage_facts() {
    let domain = product(1);
    let initial = odd_byte(&domain);
    let address = Address::repeat_byte(0x44);
    let mut account = EvmAccount::<NumericValue>::from_hex("60015f555f5ffd", Fork::Osaka).unwrap();
    account.storage.insert(U256::ZERO, initial.clone());
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "product root revert");
    world.insert(address, account).unwrap();
    let graph = native::run(
        &native::product_with(native::spec(1, 4, 1024)),
        AnalysisInput::new(
            world,
            EvmEntry {
                address,
                environment: evm_abstract::world::EvmEnvironmentValues {
                    to: (address).into(),
                    caller: (Address::repeat_byte(0x55)).into(),
                    value: word(0),
                    calldata: EvmByteArray::<NumericValue>::empty(),
                    is_static: false,
                    ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
                },
            },
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged);
    assert!(
        graph
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Revert)
    );
    let checkpoint = graph.states()[0]
        .entry
        .call_stack
        .root()
        .state
        .saved_store
        .state();
    let observed = checkpoint.read(address, &word(0), domain.borrowed());
    assert_eq!(observed.numeric(), initial.numeric());
    assert_eq!(observed.provenance(), initial.provenance());
    assert_eq!(
        checkpoint.read(address, &word(0), domain.borrowed()),
        observed
    );
    let ExprKind::SlotRead { state, key } = observed.expression().unwrap().kind() else {
        panic!("initial nonfinite value must retain a snapshot observation");
    };
    assert_eq!(key.as_constant(), Some(U256::ZERO));
    let SlotStateKind::Input { source, bindings } = state.kind() else {
        panic!("initial read must bind an observed snapshot cell");
    };
    assert_eq!(
        source.namespace(),
        U256::from_be_slice(address.as_slice()) << 8
    );
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].index, U256::ZERO);
    assert_eq!(&bindings[0].numeric, initial.numeric());
    assert_eq!(
        graph.states()[0]
            .exit
            .as_ref()
            .unwrap()
            .store
            .read(address, &word(0), domain.borrowed()),
        word(1)
    );
    // 保守的入口 gas 失败同样必须恢复完整初始 product。
    for outcome in graph.outcomes() {
        assert!(matches!(
            outcome.kind,
            OutcomeKind::Revert | OutcomeKind::Failure
        ));
        assert_eq!(
            outcome.store.read(address, &word(0), domain.borrowed()),
            observed
        );
        assert_eq!(
            outcome.store.slot_root(address, false),
            checkpoint.slot_root(address, false)
        );
        assert_eq!(observed.numeric(), initial.numeric());
        assert_eq!(observed.provenance(), initial.provenance());
    }
    ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
}

proptest! {
    #[test]
    fn product_join_obeys_semilattice_laws_without_optional_reduction(
        a in vec(any::<u16>(), 1..12),
        b in vec(any::<u16>(), 1..12),
        c in vec(any::<u16>(), 1..12),
        capacity in 1_usize..8,
    ) {
        let domain = product(capacity);
        let (left, middle, right) = (set(&domain, &a), set(&domain, &b), set(&domain, &c));
        prop_assert_eq!(domain.join(&left, &left), left.clone());
        prop_assert_eq!(domain.join(&left, &middle), domain.join(&middle, &left));
        prop_assert_eq!(domain.join(&domain.join(&left, &middle), &right), domain.join(&left, &domain.join(&middle, &right)));
        prop_assert_eq!(domain.join(&left, &EvmValue::<NumericValue>::top()), EvmValue::<NumericValue>::top());
        let combined = domain.join(&left, &middle);
        for value in a.into_iter().chain(b) {
            prop_assert!(combined.contains(U256::from(value)));
        }
    }
}
