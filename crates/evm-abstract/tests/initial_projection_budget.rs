//! Native input projection shares the execution ledger and preserves its carrier.

#[path = "support/native.rs"]
mod native;

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        AnalysisInput, EvmWorldAnalysis, ExecutionConfig, FrontierReason, Status, control::Control,
    },
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, FiniteSetAlgebra,
        NumericReduction, ValueAlgebra,
    },
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmEnvironmentValues, EvmWorld},
};

fn finite_interval(lower: U256, count: usize) -> EvmValue<FiniteConstantSet> {
    EvmValue::from_numeric(
        FiniteConstantSet::try_from_values(
            (0..count).map(|index| lower + U256::from(index)).collect(),
        )
        .unwrap(),
    )
}

fn fixture() -> (EvmWorld<FiniteConstantSet>, EvmEntry<FiniteConstantSet>) {
    let address = Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "native initial projection budget");
    world
        .insert(address, EvmAccount::from_hex("00", world.fork()).unwrap())
        .unwrap();
    let entry = EvmEntry {
        address,
        environment: EvmEnvironmentValues {
            to: address.into(),
            caller: Address::ZERO.into(),
            value: EvmValue::constant(U256::ZERO),
            calldata: EvmByteArray::empty(),
            ..EvmEnvironmentValues::default()
        },
    };
    (world, entry)
}

fn execution_policy(max_work: usize) -> ExecutionConfig {
    ExecutionConfig {
        max_work,
        ..ExecutionConfig::default()
    }
}

fn assert_initial_work_frontier(graph: &EvmWorldAnalysis<FiniteConstantSet>) {
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.states().is_empty());
    assert!(graph.outcomes().is_empty());
    let [frontier] = graph.frontiers() else {
        panic!("one complete input admission must fail atomically");
    };
    assert_eq!(frontier.from, None);
    assert_eq!(frontier.target, None);
    assert_eq!(frontier.pc, None);
    assert_eq!(frontier.reason, FrontierReason::Work);
    // Semantic blob-count validation is funded before the refused input
    // projection. No entry construction or executable work has been admitted.
    assert_eq!(
        graph.work(),
        graph
            .entry()
            .environment
            .blob_hashes
            .length
            .singleton_work()
    );
    assert!(matches!(
        evm_abstract::ssa::build_world(graph),
        Err(evm_abstract::ssa::SsaError::IncompleteAnalysis)
    ));
}

#[test]
fn initial_projection_precharge_covers_entry_and_storage_payloads() {
    // FiniteSetAlgebra receives a real complete set, rather than a product
    // interval that an old profile flag would enumerate during conversion.
    // The payload itself now supplies the work that the 100K ledger must admit.
    let values = native::finite_with(native::spec(32_768, 4, 256));
    for storage in [false, true] {
        let (mut world, mut entry) = fixture();
        let baseline = native::run(
            &values,
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(
            baseline.status(),
            Status::Converged,
            "{:?}",
            baseline.frontiers()
        );
        let value = finite_interval(U256::ZERO, 32_768);
        if storage {
            let mut account = world.account(entry.address).unwrap().clone();
            account.storage.insert(U256::ZERO, value);
            world = EvmWorld::new(Fork::Osaka, "native storage input projection");
            world.insert(entry.address, account).unwrap();
        } else {
            entry.environment.value = value;
        }
        let graph = native::run(
            &values,
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
        assert_initial_work_frontier(&graph);
    }
}

#[test]
fn unknown_calldata_stays_lazy_under_the_native_input_projection_budget() {
    let values = native::finite_with(native::spec(256, 4, 256));
    let (world, mut entry) = fixture();
    let baseline = native::run(
        &values,
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(baseline.status(), Status::Converged);
    entry.environment.calldata = EvmByteArray::unknown();
    let graph = native::run(
        &values,
        AnalysisInput::new(world, entry).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(graph.work() > 0 && graph.work() <= 100_000);
    let data = &graph.states()[0].entry.active().calldata;
    assert!(data.len().contains(U256::ZERO));
    assert!(data.len().contains(U256::MAX));
    let byte = data.byte_at(0, values.borrowed());
    for candidate in 0_u64..=255 {
        assert!(byte.contains(U256::from(candidate)));
    }
    // The finite carrier conservatively leaves an unenumerated byte as Top;
    // its immutable byte source still supplies the contextual byte semantics.
    assert!(byte.numeric().is_top());
}

#[test]
fn native_finite_projection_preserves_a_complete_range_across_a_wide_bit_boundary() {
    let values = native::finite_with(native::spec(512, 4, 256));
    let lower = (U256::ONE << 200usize) - U256::ONE;
    let upper = lower + U256::from(511);
    for max_work in [100_000, 1_000_000] {
        let (world, mut entry) = fixture();
        entry.environment.value = finite_interval(lower, 512);
        let graph = native::run(
            &values,
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(max_work),
            &Control::default(),
        )
        .unwrap();
        if max_work == 100_000 {
            assert_eq!(graph.status(), Status::Incomplete);
            assert!(
                graph
                    .frontiers()
                    .iter()
                    .any(|frontier| frontier.reason == FrontierReason::Work
                        && frontier.pc == Some(0))
            );
        } else {
            assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        }
        let candidates = graph.states()[0]
            .entry
            .active()
            .call_value
            .constants()
            .unwrap();
        assert_eq!(candidates.len(), 512);
        assert_eq!(candidates.first(), Some(&lower));
        assert_eq!(candidates.last(), Some(&upper));
        assert!(graph.work() <= max_work);
    }
}

/// A genuine finite algebra that cannot quote its input projection. The native
/// engine must refuse before calling project_numeric, even at a maximum ledger.
#[derive(Debug)]
struct UnquotedProjection(FiniteSetAlgebra);
impl EvmNumericAlgebra for UnquotedProjection {
    type Element = FiniteConstantSet;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, _value: &Self::Element) -> Self::Element {
        panic!("unpaid input projection");
    }
    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element {
        self.0.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.0.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.0.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element> {
        self.0.reduce_numeric(value)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element> {
        self.0.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Self::Element>) -> usize {
        self.0.operation_work(op, args)
    }
    fn projection_work(&self, _value: &Self::Element) -> usize {
        usize::MAX
    }
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize {
        self.0.refinement_work(value, facts)
    }
}

#[test]
fn unquotable_projection_refuses_even_with_maximum_work() {
    let values = ValueAlgebra::new(UnquotedProjection(FiniteSetAlgebra::from_spec(
        native::spec(usize::MAX, 4, 256),
    )));
    for max_work in [100_000, usize::MAX] {
        for storage in [false, true] {
            let (mut world, mut entry) = fixture();
            let value = finite_interval(U256::ZERO, 512);
            if storage {
                let mut account = world.account(entry.address).unwrap().clone();
                account.storage.insert(U256::ZERO, value);
                world = EvmWorld::new(Fork::Osaka, "unquotable native storage input");
                world.insert(entry.address, account).unwrap();
            } else {
                entry.environment.value = value;
            }
            let graph = native::run(
                &values,
                AnalysisInput::new(world, entry).unwrap(),
                execution_policy(max_work),
                &Control::default(),
            )
            .unwrap();
            assert_initial_work_frontier(&graph);
        }
    }
}

#[test]
fn maximum_capacity_top_and_small_finite_projection_remain_complete() {
    let values = native::finite_with(native::spec(usize::MAX, 4, 256));
    let finite = values.join(
        &EvmValue::constant(U256::ZERO),
        &EvmValue::constant(U256::from(2)),
    );
    for value in [EvmValue::top(), finite] {
        let (world, mut entry) = fixture();
        entry.environment.value = value;
        let graph = native::run(
            &values,
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(!graph.states().is_empty());
    }
}

#[test]
fn finite_projection_keeps_every_native_candidate_when_work_fits() {
    for capacity in [512, usize::MAX] {
        let values = native::finite_with(native::spec(capacity, 4, 256));
        let (world, mut entry) = fixture();
        entry.environment.value = finite_interval(U256::ZERO, 512);
        let graph = native::run(
            &values,
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(1_000_000),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        let candidates = graph.states()[0]
            .entry
            .active()
            .call_value
            .constants()
            .unwrap();
        assert_eq!(candidates.len(), 512);
        assert_eq!(candidates.first(), Some(&U256::ZERO));
        assert_eq!(candidates.last(), Some(&U256::from(511)));
        assert!(graph.work() > 1000 && graph.work() <= 1_000_000);
    }
}
