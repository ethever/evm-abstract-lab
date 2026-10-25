//! Actual Regional engine coverage of native Body and subscriber inputs.
//! The scoped hooks only borrow already instantiated values; no replay, native
//! Clone, singleton query, publication or analysis feedback is added.

use crate::{
    Address, Fork, U256,
    analysis::{
        AnalysisEngine, AnalysisInput, CorrelatedJoin, EnginePolicies, EvmMachinePayload,
        ExecutionConfig, NoAcceleration, RegionalSummaries, SemanticPolicies, Status,
        StructuralPartition, control::Control, cooperation::NoExtension,
    },
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, NumericReduction,
        SummaryConstruction, ValueAlgebra, WordElement, WordObservation, congruence::Congruence,
        interval::Interval, known_bits::KnownBits,
    },
    test_instrumentation::Counter,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use serde::Serialize;
use std::fmt;

const COPY: usize = 64;
const QUERY: usize = 128;

#[derive(Clone, Copy, Default, Debug)]
struct Evidence {
    child_bodies: usize,
    grandchild_bodies: usize,
    private_deliveries: usize,
}
static EVIDENCE: Counter<Evidence> = Counter::new(Evidence {
    child_bodies: 0,
    grandchild_bodies: 0,
    private_deliveries: 0,
});
static CLONES: Counter<usize> = Counter::new(0);

fn private<N: WordElement>(value: &EvmValue<N>) -> bool {
    value.numeric().requires_retained_binding()
        && value.contains(U256::from(1))
        && !value.contains(U256::from(2))
}

pub(in crate::analysis::transfer::procedure) fn body_input<N: WordElement>(
    payload: &EvmMachinePayload<N>,
) {
    EVIDENCE.update(|evidence| {
        let frame = payload.active();
        if frame.key.code_address == address(0x200) {
            assert!(
                private(&frame.call_value),
                "child Body lost the native predicate"
            );
            evidence.child_bodies += 1;
        } else if frame.key.code_address == address(0x300) {
            assert!(
                private(&frame.call_value),
                "grandchild Body lost the native predicate"
            );
            evidence.grandchild_bodies += 1;
        }
    });
}

pub(in crate::analysis::transfer::procedure) fn delivery_input<N: WordElement>(
    payload: &EvmMachinePayload<N>,
) -> bool {
    let mut observed = false;
    EVIDENCE.update(|_| {
        let mut frames = payload.call_stack.iter();
        let parent = frames.next().unwrap();
        let child = frames.next().unwrap();
        assert!(frames.next().is_none());
        assert_eq!(parent.key.code_address, address(0x200));
        assert_eq!(child.key.code_address, address(0x300));
        assert!(private(&parent.call_value), "subscriber parent lost Odd");
        assert!(private(&child.call_value), "subscriber child lost Odd");
        observed = true;
    });
    observed
}

pub(in crate::analysis::transfer::procedure) fn delivery_complete(observed: bool) {
    if observed {
        EVIDENCE.update(|evidence| evidence.private_deliveries += 1);
    }
}

// The actual carrier is retained throughout execution. Odd is a private
// predicate absent from its WordObservation. Prices/tags are immutable data.
#[derive(Debug, Serialize)]
struct Word {
    facts: WordObservation,
    odd: bool,
    tag: &'static str,
    copy: usize,
    query: usize,
}
impl Word {
    fn make(
        facts: WordObservation,
        odd: bool,
        tag: &'static str,
        copy: usize,
        query: usize,
    ) -> Self {
        Self {
            facts,
            odd,
            tag,
            copy,
            query,
        }
    }
    fn ordinary(facts: WordObservation) -> Self {
        Self::make(facts, false, "ordinary", COPY, QUERY)
    }
    fn private(copy: usize, tag: &'static str) -> Self {
        Self::make(WordObservation::top(), true, tag, copy, QUERY)
    }
}
impl Clone for Word {
    fn clone(&self) -> Self {
        CLONES.update(|count| *count += 1);
        Self::make(
            self.facts.clone(),
            self.odd,
            self.tag,
            self.copy,
            self.query,
        )
    }
}

// Representation identity and cached prices do not change the represented set.
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        self.odd == other.odd && self.facts == other.facts
    }
}
impl Eq for Word {}
impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}; odd={}", self.facts, self.odd)
    }
}
impl WordElement for Word {
    fn top() -> Self {
        Self::ordinary(WordObservation::top())
    }
    fn constant(v: U256) -> Self {
        Self::ordinary(WordObservation::constant(v))
    }
    fn from_observation(v: &WordObservation) -> Self {
        Self::ordinary(v.clone())
    }
    fn observation(&self) -> WordObservation {
        self.facts.clone()
    }
    fn requires_retained_binding(&self) -> bool {
        self.odd
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        self.facts.finite_constants()
    }
    fn known_bits(&self) -> &KnownBits {
        self.facts.known_bits()
    }
    fn interval(&self) -> &Interval {
        self.facts.interval()
    }
    fn congruence(&self) -> &Congruence {
        self.facts.congruence()
    }
    fn contains(&self, v: U256) -> bool {
        self.facts.contains(v) && (!self.odd || v.bit(0))
    }
    fn singleton(&self) -> Option<U256> {
        self.facts.singleton().filter(|v| !self.odd || v.bit(0))
    }
    fn is_top(&self) -> bool {
        !self.odd && self.facts.is_top()
    }
    fn work_size(&self) -> usize {
        self.facts.work_size().saturating_add(4)
    }
    fn clone_work(&self) -> usize {
        self.copy
    }
    fn singleton_work(&self) -> usize {
        self.query
    }
    fn canonical_for_summary(&self) -> Self {
        self.clone()
    }
}

#[derive(Debug)]
struct Algebra {
    joined_copy: usize,
}
impl Algebra {
    fn join(&self, left: &Word, right: &Word) -> Word {
        if left.odd && right.odd {
            // Both inputs in this fixture denote all odd words. This fresh,
            // deterministic representation has the SAME predicate and new fee.
            Word::private(self.joined_copy, "joined_odd")
        } else if left == right {
            Word::make(
                left.facts.clone(),
                left.odd,
                left.tag,
                left.copy,
                left.query,
            )
        } else {
            Word::top()
        }
    }
}
impl EvmNumericAlgebra for Algebra {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        DomainSpec::default()
    }
    fn project_numeric(&self, v: &Word) -> Word {
        v.clone()
    }
    fn join_numeric(&self, l: &Word, r: &Word) -> Word {
        self.join(l, r)
    }
    fn widen_numeric(&self, l: &Word, r: &Word) -> Word {
        self.join(l, r)
    }
    fn widen_procedure_numeric(&self, l: &Word, r: &Word) -> Word {
        self.join(l, r)
    }
    fn reduce_numeric(&self, v: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(v.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        let args: Vec<_> = args.collect();
        let literals: Option<Vec<_>> = args.iter().map(|v| v.facts.singleton()).collect();
        let exact = literals.as_ref().and_then(|a| match (op, a.as_slice()) {
            (0x17, [l, r]) => Some(*l | *r),
            (0x16, [l, r]) => Some(*l & *r),
            (0x18, [l, r]) => Some(*l ^ *r),
            (0x01, [l, r]) => Some(l.wrapping_add(*r)),
            (0x03, [l, r]) => Some(l.wrapping_sub(*r)),
            (0x10, [l, r]) => Some(U256::from(u8::from(l < r))),
            (0x14, [l, r]) => Some(U256::from(u8::from(l == r))),
            (0x15, [v]) => Some(U256::from(u8::from(*v == U256::ZERO))),
            (0x1b, [shift, v]) => Some(if *shift >= U256::from(256) {
                U256::ZERO
            } else {
                *v << shift.to::<usize>()
            }),
            (0x1a, [index, v]) => Some(if *index >= U256::from(32) {
                U256::ZERO
            } else {
                (*v >> ((31 - index.to::<usize>()) * 8)) & U256::from(255)
            }),
            _ => None,
        });
        let value = if let Some(v) = exact {
            Word::constant(v)
        } else if op == 0x17
            && args
                .iter()
                .any(|v| v.odd || v.facts.singleton().is_some_and(|v| v.bit(0)))
        {
            Word::private(COPY, "generated_odd")
        } else {
            Word::top()
        };
        NumericReduction::stable(value)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        // Complete finite envelope: input fees, bounded fact visits, literal /
        // Top / private-Odd result and facade queries/copies (result is cheap).
        let mut fee = 4096usize;
        for value in args {
            fee = fee
                .saturating_add(value.clone_work())
                .saturating_add(value.singleton_work())
                .saturating_add(value.work_size().saturating_mul(64));
        }
        let _ = op;
        fee
    }
    fn literal_quote_work(&self, _: U256) -> usize {
        1024
    }
    fn projection_work(&self, v: &Word) -> usize {
        v.copy
    }
    fn refinement_work(&self, v: &Word, facts: usize) -> usize {
        v.copy
            .saturating_mul(4)
            .saturating_add(v.query.saturating_mul(4))
            .saturating_add(facts.saturating_add(1).saturating_mul(1024))
    }
    fn summary_join_work(&self, l: &Word, r: &Word, _: bool) -> usize {
        // join() constructs the output directly. It does NOT clone or query
        // the returned high-fee value; that future independent copy is extra.
        l.work_size()
            .saturating_add(r.work_size())
            .saturating_mul(64)
            .saturating_add(1024)
    }
    fn summary_canonical_work(&self, v: &Word) -> usize {
        v.copy.saturating_add(64)
    }
    fn summary_construction_work(&self, _: SummaryConstruction) -> usize {
        1024
    }
    fn summary_equal_work(&self, l: &Word, r: &Word) -> usize {
        l.work_size()
            .saturating_add(r.work_size())
            .saturating_mul(64)
    }
    fn summary_is_top_work(&self, _: &Word) -> usize {
        64
    }
    fn summary_compact_work(&self, v: &Word) -> usize {
        v.copy.saturating_add(64)
    }
    fn summary_restore_work(&self, v: &Word, _: &Word) -> usize {
        v.copy.saturating_add(64)
    }
    fn summary_observation_work(&self, v: &WordObservation) -> usize {
        v.work_size().saturating_mul(64).saturating_add(1024)
    }
}

fn address(value: u16) -> Address {
    let mut bytes = [0u8; 20];
    bytes[18..].copy_from_slice(&value.to_be_bytes());
    Address::from(bytes)
}

#[test]
fn actual_regional_body_and_nested_subscriber_keep_private_oddness() {
    let mut world = EvmWorld::new(Fork::Osaka, "native Regional Body regression");
    let mut root =
        EvmAccount::from_hex("5f5f5f5f5f356001176102006207a120f15000", world.fork()).unwrap();
    root.balance = EvmValue::top();
    world.insert(address(0x101), root).unwrap();
    world
        .insert(
            address(0x200),
            EvmAccount::from_hex("5f5f5f5f346103006207a120f15000", world.fork()).unwrap(),
        )
        .unwrap();
    world
        .insert(
            address(0x300),
            EvmAccount::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::<Word>::new(address(0x101));
    assert!(entry.environment.caller.as_concrete().is_none());
    assert!(entry.environment.value.numeric().is_top());
    assert!(entry.environment.calldata.len().numeric().is_top());
    // WordObservation intentionally cannot express the actual predicate.
    let odd = Word::private(COPY, "probe");
    assert!(odd.odd && odd.facts.is_top());
    assert!(odd.contains(U256::from(1)) && !odd.contains(U256::from(2)));
    let _scope = EVIDENCE.scope();
    let algebra = ValueAlgebra::new(Algebra { joined_copy: COPY });
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        NoAcceleration,
        RegionalSummaries::with_early_known_admission(),
    ));
    let mut extension = NoExtension;
    let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: 20_000_000,
                root_terminal_coverage: false,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| record.closed && record.source_current)
    );
    let propagated: usize = result
        .procedure_summaries()
        .iter()
        .map(|record| record.propagations)
        .sum();
    let evidence = EVIDENCE.get();
    assert!(evidence.child_bodies > 0, "{evidence:?}");
    assert!(evidence.grandchild_bodies > 0, "{evidence:?}");
    assert!(evidence.private_deliveries > 0, "{evidence:?}");
    assert!(propagated >= evidence.private_deliveries);
}

#[test]
fn refused_frontier_export_moves_native_range_without_copying_or_losing_obligation() {
    use crate::analysis::{EvmRangeRequest, EvmRangeWitness, FrontierReason, RangeKind};
    use std::sync::Arc;

    // Cold setup is caller-funded. Export runs after a Work refusal and cannot
    // pay this actual native fee, even though its retained structure is small.
    let algebra = ValueAlgebra::new(Algebra { joined_copy: COPY });
    let mut world = EvmWorld::new(Fork::Osaka, "owned frontier export");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::new(address(0x101));
    let payload =
        Arc::new(crate::analysis::transfer::initial(&world, &entry, algebra.borrowed()).unwrap());
    let target = payload.key();
    let mut frontiers = vec![super::super::EvmExecutionFrontier {
        pc: 7,
        reason: FrontierReason::Work,
        target: Some(target.clone()),
        range: Some(EvmRangeWitness {
            request: EvmRangeRequest {
                kind: RangeKind::MemoryPolicy,
                offset: EvmValue::from_numeric(Word::private(usize::MAX, "range")),
                size: EvmValue::constant(U256::from(1)),
                limit: EvmValue::constant(U256::from(32)),
            },
            condition: EvmValue::constant(U256::from(1)),
            nonzero: Some(false),
            payload: Arc::clone(&payload),
        }),
        payload: Some(Arc::clone(&payload)),
    }];
    let _scope = CLONES.scope();
    let exported: Vec<_> = super::super::export_frontiers(&mut frontiers).collect();
    assert_eq!(CLONES.get(), 0);
    let range = exported[0].range.as_ref().unwrap();
    assert_eq!(range.request.offset.numeric().clone_work(), usize::MAX);
    assert!(private(&range.request.offset));
    assert!(Arc::ptr_eq(&range.payload, &payload));
    assert_eq!(frontiers.len(), 1);
    assert_eq!(frontiers[0].reason, FrontierReason::Work);
    assert_eq!(frontiers[0].target.as_ref(), Some(&target));
    assert!(frontiers[0].range.is_none());
    assert!(Arc::ptr_eq(
        frontiers[0].payload.as_ref().unwrap(),
        &payload
    ));
}

#[test]
fn native_fanout_reuses_one_private_template_and_keeps_every_target_slot() {
    use crate::analysis::transfer::procedure::fanout::{CONTROL, Counts};
    let values = ValueAlgebra::new(Algebra { joined_copy: COPY });
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        NoAcceleration,
        RegionalSummaries::with_early_known_admission(),
    ));
    let mut world = EvmWorld::new(Fork::Osaka, "opaque native jump fanout");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("5f5f5f5f6102006207a120f45000", world.fork()).unwrap(),
        )
        .unwrap();
    // The three legal targets are odd; CALLVALUE has a private Odd predicate
    // absent from its public WordObservation. PC2 is an unreachable STOP.
    world
        .insert(
            address(0x200),
            EvmAccount::from_hex("3456005b005b005b00", world.fork()).unwrap(),
        )
        .unwrap();
    let mut entry = EvmEntry::<Word>::new(address(0x101));
    entry.environment.value = EvmValue::from_numeric(Word::private(COPY, "fanout"));
    let input = AnalysisInput::new(world, entry).unwrap();
    let _control = CONTROL.scope();
    let _evidence = EVIDENCE.scope();
    let run = |enabled| {
        CONTROL.set(Counts {
            enabled,
            forms: 0,
            copies: 0,
        });
        EVIDENCE.set(Evidence::default());
        let mut extension = NoExtension;
        let result = AnalysisEngine::new(&values, &policies, &mut extension)
            .run(
                input.clone(),
                ExecutionConfig {
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
                &Control::default(),
            )
            .unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(result.frontiers().is_empty());
        let record = result.procedure_summaries().last().unwrap();
        assert!(record.closed && record.source_current);
        let child = record
            .procedures
            .iter()
            .find(|p| p.frame.code_address == address(0x200))
            .unwrap();
        assert_eq!(child.covered_blocks, [0, 2, 3, 4]);
        assert!(
            child
                .outputs
                .iter()
                .any(|o| o.kind == crate::analysis::OutcomeKind::Failure)
        );
        assert!(
            child
                .outputs
                .iter()
                .any(|o| o.kind == crate::analysis::OutcomeKind::Return)
        );
        assert_eq!(EVIDENCE.get().child_bodies, 4);
        (result, CONTROL.get())
    };
    let (ordinary, baseline) = run(false);
    let (shared, actual) = run(true);
    assert_eq!(baseline.forms, 0);
    assert_eq!(actual.forms, 1);
    assert_eq!(actual.copies, 3);
    assert_eq!(
        &*ordinary.procedure_summaries().last().unwrap().regional_cfg,
        &*shared.procedure_summaries().last().unwrap().regional_cfg
    );
    assert!(shared.work() < ordinary.work());
    eprintln!(
        "native fanout opaque control: ordinary={} shared={} forms={} copies={}",
        ordinary.work(),
        shared.work(),
        actual.forms,
        actual.copies
    );
}

#[test]
fn native_fanout_taken_predicate_excludes_the_false_continuation() {
    use crate::analysis::{EdgeKind, MachineEdgeKind};
    let values = crate::analysis::testing::product(DomainSpec::default());
    let (world, mut entry) =
        crate::analysis::machine::fixtures::fixture(&[(0x101, "5f358060203557005b005b00")]);
    entry.environment.calldata = crate::world::EvmByteArray::unknown();
    let domain = values.borrowed();
    let payload = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    let mut budget = crate::resource::WorkBudget::new(20_000_000);
    let execution = super::super::execute_inner(
        &world,
        &entry,
        payload,
        &ExecutionConfig::default(),
        domain,
        &mut budget,
        crate::analysis::transfer::StorageReadPolicy::Abstract,
        true,
    );
    assert!(execution.frontiers.is_empty());
    let common = execution.common_successors.unwrap();
    assert_eq!(common, 0..2);
    assert_eq!(execution.successors.len(), 3);
    assert!(
        execution
            .outcomes
            .iter()
            .any(|o| o.kind == crate::analysis::OutcomeKind::Failure)
    );
    for (ordinal, successor) in execution.successors.iter().enumerate() {
        let refined = crate::analysis::state::payload::caller::copy_with_scalar_guarantees(
            &successor.payload,
            &entry,
            domain,
            &mut budget,
        )
        .unwrap();
        let condition = &refined.active().stack[0];
        if common.contains(&ordinal) {
            assert_eq!(
                successor.kind,
                MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)
            );
            assert!(!condition.contains(U256::ZERO));
        } else {
            assert_eq!(
                successor.kind,
                MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse)
            );
            assert_eq!(condition.singleton(), Some(U256::ZERO));
        }
    }
}

#[test]
fn native_fanout_does_not_group_a_child_failure_resuming_its_parent() {
    use crate::analysis::{EvmChildFrame, EvmContinuation, MachineEdgeKind};
    let values = crate::analysis::testing::product(DomainSpec::default());
    let domain = values.borrowed();
    let (world, mut entry) =
        crate::analysis::machine::fixtures::fixture(&[(0x101, "5f358060203557005b005b00")]);
    entry.environment.calldata = crate::world::EvmByteArray::unknown();
    let mut payload = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    let mut child = payload.active().clone();
    child.key.caller = child.key.address.into();
    child.environment_calldata = false;
    payload.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(0),
            output_offset: EvmValue::constant(U256::ZERO),
            output_size: EvmValue::constant(U256::ZERO),
            creation: None,
        },
    });
    let mut budget = crate::resource::WorkBudget::new(20_000_000);
    let execution = super::super::execute_inner(
        &world,
        &entry,
        payload,
        &ExecutionConfig::default(),
        domain,
        &mut budget,
        crate::analysis::transfer::StorageReadPolicy::Abstract,
        false,
    );
    assert!(execution.frontiers.is_empty());
    assert!(execution.common_successors.is_none());
    assert!(execution.successors.iter().any(|successor| {
        successor.kind == MachineEdgeKind::Failure && successor.payload.call_stack.depth() == 1
    }));
    assert!(execution.successors.iter().any(|successor| {
        matches!(
            successor.kind,
            MachineEdgeKind::Intraprocedural(crate::analysis::EdgeKind::BranchTrue)
        ) && successor.payload.call_stack.depth() == 2
    }));
}
