//! Dispatch witnesses use an independent interval carrier with private facts.

use super::*;
use crate::domain::ProductAlgebra;
use crate::{
    Fork,
    analysis::{
        EdgeKind, ExecutionConfig, MachineEdgeKind, OutcomeKind, RecentDistinctJumps,
        transfer::{self, TransferContext, WorkBudget, empty_execution},
    },
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, ValueAlgebra, WordObservation,
        facts::{FactError, Symbol},
        relational::QueryReason,
        symbolic::ExprId,
    },
    test_instrumentation::Counter,
    world::{AddressInput, EvmAccount, EvmByteArray, EvmEntry, EvmWorld, Existence},
};
use alloy_primitives::Address;
use revm_bytecode::opcode;
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Copy)]
struct Receipt {
    fee: usize,
    watch_word: Option<U256>,
    watch_arithmetic: bool,
    mask_fee: usize,
    mask_quotes: usize,
    mask_factories: usize,
    mask_quote_cancel: bool,
    mask_factory_cancel: bool,
    quotes: usize,
    queries: usize,
    membership: usize,
    on_quote: bool,
    on_query: bool,
    on_membership: bool,
}

static RECEIPT: Counter<Receipt> = Counter::new(Receipt {
    fee: 313,
    watch_word: None,
    watch_arithmetic: false,
    mask_fee: 64,
    mask_quotes: 0,
    mask_factories: 0,
    mask_quote_cancel: false,
    mask_factory_cancel: false,
    quotes: 0,
    queries: 0,
    membership: 0,
    on_quote: false,
    on_query: false,
    on_membership: false,
});

#[derive(Clone, Debug, Serialize)]
struct Opaque {
    low: U256,
    high: U256,
    excluded: Option<U256>,
    finite: Option<BTreeSet<U256>>,
    #[serde(skip)]
    watched: bool,
    #[serde(skip)]
    cancel_owner: Option<analysis_control::Cancellation>,
}

impl PartialEq for Opaque {
    fn eq(&self, other: &Self) -> bool {
        (self.low, self.high, self.excluded, &self.finite)
            == (other.low, other.high, other.excluded, &other.finite)
    }
}
impl Eq for Opaque {}

impl Opaque {
    fn is_watched(&self) -> bool {
        let mut watched = self.watched;
        RECEIPT.update(|receipt| {
            watched |= receipt
                .watch_word
                .is_some_and(|word| self.exact() == Some(word))
        });
        watched
    }
    fn exact(&self) -> Option<U256> {
        (self.low == self.high && self.excluded != Some(self.low)).then_some(self.low)
    }
    fn cancel(&self) {
        if let Some(owner) = &self.cancel_owner {
            owner.cancel();
        } else {
            analysis_control::current_cancellation().cancel();
        }
    }
}

impl fmt::Display for Opaque {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={} except {:?}", self.low, self.high, self.excluded)
    }
}

impl WordElement for Opaque {
    fn top() -> Self {
        Self {
            low: U256::ZERO,
            high: U256::MAX,
            excluded: None,
            finite: None,
            watched: false,
            cancel_owner: None,
        }
    }
    fn constant(word: U256) -> Self {
        if word == U256::MAX >> 96usize {
            RECEIPT.update(|receipt| {
                receipt.mask_factories += 1;
                if receipt.mask_factory_cancel {
                    analysis_control::current_cancellation().cancel();
                }
            });
        }
        Self {
            low: word,
            high: word,
            ..Self::top()
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        let (low, high) = facts.interval().unsigned_bounds();
        Self {
            low,
            high,
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        WordObservation::unsigned_range(self.low, self.high).unwrap()
    }
    fn constrain(&self, facts: &WordObservation) -> Result<Self, FactError> {
        let (low, high) = facts.interval().unsigned_bounds();
        let next = Self {
            low: self.low.max(low),
            high: self.high.min(high),
            ..self.clone()
        };
        if next.low > next.high || next.low == next.high && next.excluded == Some(next.low) {
            Err(FactError::Contradiction {
                subject: Symbol::THIS,
            })
        } else {
            Ok(next)
        }
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.finite.as_ref()
    }
    fn contains(&self, word: U256) -> bool {
        if self.is_watched() {
            RECEIPT.update(|receipt| {
                receipt.membership += 1;
                if receipt.on_membership {
                    self.cancel();
                }
            });
        }
        self.low <= word && word <= self.high && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        if self.is_watched() {
            RECEIPT.update(|receipt| {
                receipt.queries += 1;
                if receipt.on_query {
                    self.cancel();
                }
            });
        }
        self.exact()
    }
    fn singleton_work(&self) -> usize {
        if !self.is_watched() {
            return 1;
        }
        let mut fee = 1;
        RECEIPT.update(|receipt| {
            receipt.quotes += 1;
            fee = receipt.fee;
            if receipt.on_quote {
                self.cancel();
            }
        });
        fee
    }
    fn is_top(&self) -> bool {
        self.low == U256::ZERO && self.high == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        64
    }
}

#[derive(Debug)]
struct Policy {
    spec: DomainSpec,
}

impl EvmNumericAlgebra for Policy {
    type Element = Opaque;
    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, value: &Opaque) -> Opaque {
        value.clone()
    }
    fn join_numeric(&self, a: &Opaque, b: &Opaque) -> Opaque {
        Opaque {
            low: a.low.min(b.low),
            high: a.high.max(b.high),
            watched: a.watched || b.watched,
            ..Opaque::top()
        }
    }
    fn widen_numeric(&self, a: &Opaque, b: &Opaque) -> Opaque {
        self.join_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &Opaque, b: &Opaque) -> Opaque {
        self.join_numeric(a, b)
    }
    fn reduce_numeric(&self, value: &Opaque) -> NumericReduction<Opaque> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Opaque>,
    ) -> NumericReduction<Opaque> {
        let args: Vec<&Opaque> = args.collect();
        // No calls to the instrumented singleton interface inside this algebra.
        let words = args
            .iter()
            .copied()
            .map(Opaque::exact)
            .collect::<Option<Vec<_>>>();
        let exact = words
            .as_ref()
            .and_then(|words| match (op, words.as_slice()) {
                (opcode::AND, [a, b]) => Some(*a & *b),
                (opcode::OR, [a, b]) => Some(*a | *b),
                (opcode::ADD, [a, b]) => Some(a.wrapping_add(*b)),
                (opcode::SUB, [a, b]) => Some(a.wrapping_sub(*b)),
                (opcode::EQ, [a, b]) => Some(U256::from(u8::from(a == b))),
                (opcode::GT, [a, b]) => Some(U256::from(u8::from(a > b))),
                (opcode::LT, [a, b]) => Some(U256::from(u8::from(a < b))),
                (opcode::ISZERO, [a]) => Some(U256::from(u8::from(a.is_zero()))),
                _ => None,
            });
        let mut out = exact.map_or_else(Opaque::top, Opaque::constant);
        if op == opcode::AND && args.first().is_some_and(|arg| arg.watched) {
            out.watched = true;
            out.cancel_owner = args[0].cancel_owner.clone();
        }
        let mut watch_arithmetic = false;
        RECEIPT.update(|receipt| watch_arithmetic = receipt.watch_arithmetic);
        if watch_arithmetic && matches!(op, opcode::ADD | opcode::SUB) {
            out.watched = true;
        }
        NumericReduction::stable(out)
    }
    // Every supported scalar opcode uses the same fixed-size carrier envelope.
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Opaque>) -> usize {
        4096
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        let mut fee = 64;
        if literal == U256::MAX >> 96usize {
            RECEIPT.update(|receipt| {
                receipt.mask_quotes += 1;
                fee = receipt.mask_fee;
                if receipt.mask_quote_cancel {
                    analysis_control::current_cancellation().cancel();
                }
            });
        }
        fee
    }
    fn projection_work(&self, _value: &Opaque) -> usize {
        64
    }
    fn refinement_work(&self, _value: &Opaque, facts: usize) -> usize {
        (facts + 1) * 4096
    }
}

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn word(number: u64) -> EvmValue<Opaque> {
    EvmValue::constant(U256::from(number))
}
fn watched(number: u64) -> EvmValue<Opaque> {
    EvmValue::from_numeric(Opaque {
        watched: true,
        ..Opaque::constant(U256::from(number))
    })
}

struct Fixture {
    world: EvmWorld<Opaque>,
    entry: EvmEntry<Opaque>,
    config: ExecutionConfig,
    algebra: ValueAlgebra<Policy>,
    input: crate::analysis::EvmMachinePayload<Opaque>,
}

fn fixture(code: &str, relations: bool) -> Fixture {
    let config = ExecutionConfig {
        max_work: 1_000_000_000,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = relations;
    let algebra = ValueAlgebra::new(Policy {
        spec: ProductAlgebra::from_spec(config_numeric.spec()).spec(),
    });
    let mut world = EvmWorld::new(Fork::Osaka, "paid dispatch opaque carrier");
    let mut owner = EvmAccount::from_hex(code, world.fork()).unwrap();
    owner.balance = word(100);
    owner.nonce = word(0);
    world.insert(address(0x101), owner).unwrap();
    let mut target = EvmAccount::empty();
    target.balance = word(50);
    target.nonce = word(0);
    world.insert(address(0x202), target).unwrap();
    let entry = EvmEntry::concrete(
        address(0x101),
        address(0x303),
        word(0),
        EvmByteArray::empty(),
    );
    let input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    Fixture {
        world,
        entry,
        config,
        algebra,
        input,
    }
}

fn context<'a>(fixture: &'a Fixture, budget: &'a mut WorkBudget) -> TransferContext<'a, Policy> {
    transfer::testing::procedure_context(&fixture.config, budget, fixture.algebra.borrowed())
}

fn run(fixture: &Fixture, budget: &mut WorkBudget) -> transfer::EvmExecution<Opaque> {
    transfer::execute_inner(
        &fixture.world,
        &fixture.entry,
        fixture.input.clone(),
        &fixture.config,
        fixture.algebra.borrowed(),
        &RecentDistinctJumps,
        budget,
        transfer::StorageReadPolicy::Abstract,
        true,
    )
}

#[track_caller]
fn assert_stop_frontier(
    result: &transfer::EvmExecution<Opaque>,
    reason: FrontierReason,
    label: &str,
) {
    assert_eq!(result.frontiers.len(), 1, "{label}");
    assert_eq!(result.frontiers[0].reason, reason, "{label}");
    assert!(result.completed_calls.is_empty(), "{label}");
}

/// Use only before any alternative has completed: even a spurious Failure is
/// a publication bug here. Later stops must describe their prior evidence.
#[track_caller]
fn assert_stopped(result: &transfer::EvmExecution<Opaque>, reason: FrontierReason) {
    assert_stop_frontier(result, reason, "strict stop before publication");
    assert!(result.successors.is_empty());
    assert!(result.outcomes.is_empty());
}

/// Expected semantic classes, independent of duplicate emission by different
/// memory checks. A root fault rolls back; a rejected call resumes its caller;
/// a rejected creation additionally preserves the creator nonce increment.
#[derive(Clone, Copy, Debug)]
enum PublishedBeforeStop {
    Nothing,
    RootFault,
    RootFaultAndCallRejection,
    RootFaultAndCreationRejection,
}

#[track_caller]
fn assert_stopped_after_failures(
    result: &transfer::EvmExecution<Opaque>,
    reason: FrontierReason,
    fixture: &Fixture,
    published: PublishedBeforeStop,
    label: &str,
) {
    assert_stop_frontier(result, reason, label);
    let root_fault = !matches!(published, PublishedBeforeStop::Nothing);
    assert_eq!(
        result.outcomes.is_empty(),
        !root_fault,
        "{label}: root fault class"
    );
    // Several pre-effect memory checks may emit the same (kind, data, store).
    // Verify the normalized semantic set: require this class when expected and
    // reject every other class, without requiring duplicate multiplicity.
    for outcome in &result.outcomes {
        assert_eq!(outcome.kind, OutcomeKind::Failure, "{label}");
        assert_eq!(outcome.data, EvmByteArray::empty(), "{label}");
        assert_eq!(
            outcome.store,
            *fixture.input.active().saved_store.state(),
            "{label}: the completed root fault restores its checkpoint"
        );
    }
    let has_resume = matches!(
        published,
        PublishedBeforeStop::RootFaultAndCallRejection
            | PublishedBeforeStop::RootFaultAndCreationRejection
    );
    assert_eq!(
        result.successors.is_empty(),
        !has_resume,
        "{label}: caller rejection class"
    );
    if has_resume {
        let mut expected = fixture.input.clone();
        if matches!(
            published,
            PublishedBeforeStop::RootFaultAndCreationRejection
        ) {
            expected.store.touch_account(address(0x101));
            expected.store.write_nonce(address(0x101), word(1));
        }
        let frame = expected.active_mut();
        frame.stack.push(word(0));
        frame.returndata = EvmByteArray::empty();
        frame.key.basic_block_index += 1;
        expected.normalize();
        for successor in &result.successors {
            assert_eq!(successor.kind, MachineEdgeKind::Failure, "{label}");
            assert_eq!(successor.payload, expected, "{label}");
        }
    }
}

struct QueryStopCase<'a> {
    site: &'a str,
    published: PublishedBeforeStop,
    watch_arithmetic: bool,
    empty_stack: bool,
}

impl<'a> QueryStopCase<'a> {
    fn new(site: &'a str, published: PublishedBeforeStop) -> Self {
        Self {
            site,
            published,
            watch_arithmetic: false,
            empty_stack: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum QueryStop {
    MaximumQuote,
    ShortBudget,
    QuoteCancelled,
    QueryCancelled,
}

/// Every site keeps the same four payment boundaries. The maximum-quote run
/// measures only the paid prefix, so structural fee changes cannot silently
/// move the one-short run to an earlier phase. 313 is this carrier's chosen
/// quote; 312 refuses that query by one, independently of production tariffs.
fn assert_query_stops(
    fixture: &Fixture,
    case: QueryStopCase<'_>,
    execute: impl Fn(&mut WorkBudget) -> transfer::EvmExecution<Opaque>,
) {
    let mut prefix = 0;
    for mode in [
        QueryStop::MaximumQuote,
        QueryStop::ShortBudget,
        QueryStop::QuoteCancelled,
        QueryStop::QueryCancelled,
    ] {
        let label = format!("{}: {mode:?}", case.site);
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.watch_arithmetic = case.watch_arithmetic;
            receipt.fee = if matches!(mode, QueryStop::MaximumQuote) {
                usize::MAX
            } else {
                313
            };
            receipt.on_quote = matches!(mode, QueryStop::QuoteCancelled);
            receipt.on_query = matches!(mode, QueryStop::QueryCancelled);
        });
        let mut budget = WorkBudget::new(if matches!(mode, QueryStop::ShortBudget) {
            prefix + 312
        } else {
            usize::MAX
        });
        let token = analysis_control::Cancellation::new();
        let result = analysis_control::with_cancellation(&token, || execute(&mut budget));
        if matches!(mode, QueryStop::MaximumQuote) {
            prefix = budget.used();
        }
        let queried = matches!(mode, QueryStop::QueryCancelled);
        let cancelled = matches!(mode, QueryStop::QuoteCancelled | QueryStop::QueryCancelled);
        assert_stopped_after_failures(
            &result,
            if cancelled {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            },
            fixture,
            case.published,
            &label,
        );
        assert_eq!(RECEIPT.get().quotes, 1, "{label}");
        assert_eq!(RECEIPT.get().queries, usize::from(queried), "{label}");
        assert_eq!(
            budget.used(),
            prefix + if queried { 313 } else { 0 },
            "{label}"
        );
        assert_eq!(result.payload.store, fixture.input.store, "{label}");
        assert_eq!(
            result.payload.active().memory,
            fixture.input.active().memory,
            "{label}"
        );
        if case.empty_stack {
            assert!(
                result.payload.active().stack.is_empty(),
                "{label}: consumed operand must not publish a hash"
            );
        }
    }
}

#[test]
fn jump_pays_exact_opaque_destination_and_keeps_its_only_valid_edge() {
    let fixture = fixture("565b005b00", false);
    let mut fixture = fixture;
    fixture.input.active_mut().stack = vec![watched(3)];
    let _receipt = RECEIPT.scope();
    let mut budget = WorkBudget::new(fixture.config.max_work);
    let result = run(&fixture, &mut budget);
    assert!(result.frontiers.is_empty());
    assert!(result.outcomes.is_empty());
    assert_eq!(result.successors.len(), 1);
    let successor = &result.successors[0];
    assert_eq!(
        successor.kind,
        MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
    );
    assert_eq!(
        successor.payload.active().key.basic_block_index,
        fixture
            .input
            .active()
            .program
            .as_ref()
            .unwrap()
            .jumpdest_blocks()[&3]
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    let complete = budget.used();
    RECEIPT.update(|receipt| {
        receipt.queries = 0;
        receipt.fee = usize::MAX;
    });
    let mut unlimited = WorkBudget::new(usize::MAX);
    let stopped = run(&fixture, &mut unlimited);
    assert_stopped(&stopped, FrontierReason::Work);
    assert_eq!(RECEIPT.get().queries, 0);
    let prefix = unlimited.used();
    assert!(prefix < complete);
    RECEIPT.update(|receipt| receipt.fee = 313);
    let mut short = WorkBudget::new(prefix + 312);
    assert_stopped(&run(&fixture, &mut short), FrontierReason::Work);
    assert_eq!((RECEIPT.get().queries, short.used()), (0, prefix));
}

#[track_caller]
fn assert_jumpi_successors(
    result: &transfer::EvmExecution<Opaque>,
    fixture: &Fixture,
    expected: &[(EdgeKind, usize)],
) {
    assert!(result.frontiers.is_empty());
    assert!(result.outcomes.is_empty());
    assert!(result.completed_calls.is_empty());
    assert_eq!(result.successors.len(), expected.len());
    for &(kind, target) in expected {
        let destinations = result
            .successors
            .iter()
            .filter(|successor| successor.kind == MachineEdgeKind::Intraprocedural(kind))
            .map(|successor| successor.payload.active().key.basic_block_index)
            .collect::<Vec<_>>();
        assert_eq!(
            destinations,
            [fixture
                .input
                .active()
                .program
                .as_ref()
                .unwrap()
                .jumpdest_blocks()[&target]],
            "JUMPI must retain the {kind:?} edge to PC {target} exactly once"
        );
    }
}

#[test]
fn jumpi_reuses_condition_answer_and_preserves_arbitrary_nonzero_and_zero_edges() {
    for (condition, expected) in [
        (watched(7), (EdgeKind::BranchTrue, 3)),
        (watched(0), (EdgeKind::BranchFalse, 1)),
    ] {
        let mut fixture = fixture("575b005b00", false);
        fixture.input.active_mut().stack = vec![condition, word(3)];
        let _receipt = RECEIPT.scope();
        let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
        assert_jumpi_successors(&result, &fixture, &[expected]);
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    }
    let mut fixture = fixture("575b005b00", false);
    fixture.input.active_mut().stack = vec![
        EvmValue::from_numeric(Opaque {
            watched: true,
            low: U256::ZERO,
            high: U256::from(7),
            ..Opaque::top()
        }),
        word(3),
    ];
    let _receipt = RECEIPT.scope();
    let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
    assert_jumpi_successors(
        &result,
        &fixture,
        &[(EdgeKind::BranchTrue, 3), (EdgeKind::BranchFalse, 1)],
    );
    assert_eq!(
        (
            RECEIPT.get().quotes,
            RECEIPT.get().queries,
            RECEIPT.get().membership
        ),
        (1, 1, 1)
    );
}

#[test]
fn jumpi_zero_never_discovers_destination_and_opaque_exclusion_prunes_zero() {
    let mut fixture = fixture("575b005b00", false);
    fixture.input.active_mut().stack = vec![word(0), watched(3)];
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
    assert!(result.frontiers.is_empty());
    assert_eq!(result.successors.len(), 1);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    RECEIPT.update(|receipt| receipt.fee = 313);
    fixture.input.active_mut().stack = vec![
        EvmValue::from_numeric(Opaque {
            watched: true,
            excluded: Some(U256::ZERO),
            ..Opaque::top()
        }),
        word(3),
    ];
    let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
    assert!(result.frontiers.is_empty());
    assert_eq!(result.successors.len(), 1);
    assert_eq!(
        result.successors[0].kind,
        MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)
    );
}

#[test]
fn jumpi_quote_query_and_membership_cancellation_publish_no_edges_or_state_effects() {
    for point in 0..3 {
        for captured in [false, true] {
            let mut fixture = fixture("575b005b00", false);
            let owner = analysis_control::Cancellation::new();
            fixture.input.active_mut().stack = vec![
                EvmValue::from_numeric(Opaque {
                    watched: true,
                    cancel_owner: captured.then(|| owner.clone()),
                    ..Opaque::top()
                }),
                word(3),
            ];
            let before = fixture.input.store.clone();
            let _receipt = RECEIPT.scope();
            RECEIPT.update(|receipt| {
                receipt.on_quote = point == 0;
                receipt.on_query = point == 1;
                receipt.on_membership = point == 2;
            });
            let mut budget = analysis_control::with_cancellation(&owner, || {
                WorkBudget::new(fixture.config.max_work)
            });
            let active = analysis_control::Cancellation::new();
            let result =
                analysis_control::with_cancellation(&active, || run(&fixture, &mut budget));
            assert_stopped(&result, FrontierReason::Relations(QueryReason::Cancelled));
            assert_eq!(result.payload.store, before);
            assert_eq!(RECEIPT.get().queries, usize::from(point != 0));
            assert_eq!(RECEIPT.get().membership, usize::from(point == 2));
        }
    }
}

#[test]
fn cached_cover_literal_and_saved_negative_proof_avoid_repeat_discovery() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let finite = EvmValue::from_numeric(Opaque {
        watched: true,
        finite: Some([U256::from(7)].into()),
        ..Opaque::constant(U256::from(7))
    });
    let literal = watched(7).with_expression(ExprId::constant(U256::from(7)));
    for value in [&finite, &literal] {
        let truth = Truth::prepare(
            value,
            SingletonProof::NotQueried,
            &mut WorkBudget::new(1000),
        )
        .unwrap();
        assert!(!truth.zero && truth.nonzero);
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    let unknown = EvmValue::from_numeric(Opaque {
        watched: true,
        ..Opaque::top()
    });
    let mut budget = WorkBudget::new(64);
    let truth = Truth::prepare(&unknown, SingletonProof::Answer(None), &mut budget).unwrap();
    assert!(truth.zero && truth.nonzero);
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (0, 0, 64)
    );
}

fn call_args(value: EvmValue<Opaque>, target: EvmValue<Opaque>) -> Vec<EvmValue<Opaque>> {
    vec![
        word(1_000_000),
        target,
        value,
        word(0),
        word(0),
        word(0),
        word(0),
    ]
}

fn call(
    fixture: &Fixture,
    args: &[EvmValue<Opaque>],
    budget: &mut WorkBudget,
) -> transfer::EvmExecution<Opaque> {
    let mut result = empty_execution(fixture.input.clone());
    transfer::calls::call(
        &mut result,
        &fixture.world,
        opcode::CALL,
        args,
        fixture.input.active().program.as_ref().unwrap(),
        &mut context(fixture, budget),
        0,
    );
    result
}

#[test]
fn call_paid_value_and_balance_preserve_transfer_touch_and_revert_witnesses() {
    for amount in [0, 3] {
        for watch_balance in [false, true] {
            let mut fixture = fixture("f100", false);
            if watch_balance {
                fixture
                    .input
                    .store
                    .write_balance(address(0x101), watched(100));
            }
            let args = call_args(
                if watch_balance {
                    word(amount)
                } else {
                    watched(amount)
                },
                word(0x202),
            );
            let _receipt = RECEIPT.scope();
            let mut budget = WorkBudget::new(fixture.config.max_work);
            let result = call(&fixture, &args, &mut budget);
            assert!(result.frontiers.is_empty());
            let child = result
                .successors
                .into_iter()
                .find(|successor| successor.kind == MachineEdgeKind::Call)
                .unwrap()
                .payload;
            assert_eq!(
                child.active().key.address_value,
                AddressInput::Concrete(address(0x202))
            );
            assert_eq!(
                child.store.read_balance(address(0x101)).numeric().exact(),
                Some(U256::from(100 - amount))
            );
            assert_eq!(
                child.store.read_balance(address(0x202)).numeric().exact(),
                Some(U256::from(50 + amount))
            );
            assert_eq!(
                child.store.touched_in_transaction(address(0x202)),
                Some(true)
            );
            if amount == 0 {
                assert_eq!(
                    child.store.read_balance(address(0x101)),
                    fixture.input.store.read_balance(address(0x101))
                );
            }
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                if watch_balance && amount == 0 {
                    (0, 0)
                } else {
                    (1, 1)
                }
            );
            let saved = child.active().saved_store.state().clone();
            let mut resumed = empty_execution(child.clone());
            transfer::calls::finish(
                &mut resumed,
                child,
                OutcomeKind::Revert,
                EvmByteArray::empty(),
                &mut context(&fixture, &mut budget),
                1,
            );
            let parent = &resumed.successors[0].payload;
            assert_eq!(parent.store, saved);
            assert_eq!(
                parent.active().stack.last().unwrap().numeric().exact(),
                Some(U256::ZERO)
            );
        }
    }
}

#[test]
fn call_projection_reuses_the_actual_prepaid_result_and_still_enters_exact_target() {
    for relations in [false, true] {
        let fixture = fixture("f100", relations);
        let _receipt = RECEIPT.scope();
        let result = call(
            &fixture,
            &call_args(word(0), watched(0x202)),
            &mut WorkBudget::new(fixture.config.max_work),
        );
        assert!(result.frontiers.is_empty());
        let child = result
            .successors
            .iter()
            .find(|successor| successor.kind == MachineEdgeKind::Call)
            .unwrap();
        assert_eq!(child.payload.active().key.code_address, address(0x202));
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries),
            (usize::from(!relations), 1)
        );
    }
}

#[test]
fn call_refused_or_cancelled_value_never_changes_accounts_or_publishes_a_child() {
    for point in 0..4 {
        let fixture = fixture("f100", false);
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.fee = if point == 0 { usize::MAX } else { 313 };
            receipt.on_quote = point == 2;
            receipt.on_query = point == 3;
        });
        let owner = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(if point == 1 {
            312
        } else {
            fixture.config.max_work
        });
        let result = analysis_control::with_cancellation(&owner, || {
            call(&fixture, &call_args(watched(3), word(0x202)), &mut budget)
        });
        assert_stopped(
            &result,
            if point >= 2 {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            },
        );
        assert_eq!(result.payload.store, fixture.input.store);
        assert_eq!(
            result.payload.active().memory,
            fixture.input.active().memory
        );
        assert_eq!(RECEIPT.get().queries, usize::from(point == 3));
        assert_eq!(budget.used(), if point == 3 { 313 } else { 0 });
    }
}

#[test]
fn selfdestruct_pays_beneficiary_and_keeps_zero_and_nonzero_existence_semantics() {
    for amount in [0, 3] {
        let mut fixture = fixture("ff", false);
        fixture
            .input
            .store
            .write_balance(address(0x101), word(amount));
        fixture.input.active_mut().stack = vec![watched(0x404)];
        let _receipt = RECEIPT.scope();
        let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
        assert!(result.frontiers.is_empty());
        let returned = result
            .outcomes
            .iter()
            .find(|outcome| outcome.kind == OutcomeKind::Return)
            .unwrap();
        assert_eq!(
            returned
                .store
                .read_balance(address(0x101))
                .numeric()
                .exact(),
            Some(U256::ZERO)
        );
        assert_eq!(
            returned.store.touched_in_transaction(address(0x404)),
            Some(true)
        );
        assert_eq!(
            returned.store.existence(address(0x404)),
            if amount == 0 {
                Existence::Unknown
            } else {
                Existence::Present
            }
        );
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    }
}

fn create(
    fixture: &Fixture,
    op: u8,
    args: &[EvmValue<Opaque>],
    budget: &mut WorkBudget,
) -> transfer::EvmExecution<Opaque> {
    let mut result = empty_execution(fixture.input.clone());
    transfer::create::create(
        &mut result,
        &fixture.world,
        op,
        args,
        fixture.input.active().program.as_ref().unwrap(),
        &mut context(fixture, budget),
        0,
    );
    result
}

#[test]
fn create_and_create2_discover_inputs_once_and_preserve_endowment_and_nonce_rollback() {
    for op in [opcode::CREATE, opcode::CREATE2] {
        for site in 0..9 {
            if op == opcode::CREATE && site == 7 {
                continue;
            }

            let mut fixture = fixture(if op == opcode::CREATE { "f000" } else { "f500" }, false);
            let initcode: &[u8] = if site == 8 { &[0] } else { &[] };
            let destination = if op == opcode::CREATE {
                address(0x101).create(0)
            } else {
                address(0x101).create2_from_code(U256::from(9).to_be_bytes::<32>(), initcode)
            };
            let mut account = EvmAccount::empty();
            account.balance = word(50);
            account.nonce = word(0);
            fixture.world.insert(destination, account).unwrap();
            fixture.input =
                transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed())
                    .unwrap();
            let mut args = vec![word(3), word(0), word(0), word(9)];
            match site {
                0 => args[0] = watched(3),
                1 => args[2] = watched(0),
                2 => fixture.input.active_mut().address_operand = Some(watched(0x101)),
                3 => fixture.input.store.write_nonce(address(0x101), watched(0)),
                4 => fixture
                    .input
                    .store
                    .write_balance(address(0x101), watched(100)),
                5 => fixture.input.store.write_nonce(destination, watched(0)),
                6 => fixture.input.store.write_balance(destination, watched(50)),
                7 => args[3] = watched(9),
                8 => {
                    args[1] = watched(1);
                    args[2] = word(1);
                }
                _ => unreachable!(),
            }
            let _receipt = RECEIPT.scope();
            let mut budget = WorkBudget::new(fixture.config.max_work);
            let result = create(&fixture, op, &args, &mut budget);
            assert!(result.frontiers.is_empty());
            let child = result
                .successors
                .into_iter()
                .find(|successor| successor.kind == MachineEdgeKind::Call)
                .unwrap()
                .payload;
            assert_eq!(child.active().key.address, destination);
            assert_eq!(
                child.store.read_balance(address(0x101)).numeric().exact(),
                Some(U256::from(97))
            );
            assert_eq!(
                child.store.read_balance(destination).numeric().exact(),
                Some(U256::from(53))
            );
            assert_eq!(
                child.store.nonce(address(0x101)).numeric().exact(),
                Some(U256::ONE)
            );
            assert_eq!(child.store.created_in_transaction(destination), Some(true));
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (1, 1),
                "paid input discovery is reused through all later phases: opcode {op}, site {site}",
            );
            let mut resumed = empty_execution(child.clone());
            transfer::calls::finish(
                &mut resumed,
                child,
                OutcomeKind::Revert,
                EvmByteArray::empty(),
                &mut context(&fixture, &mut budget),
                1,
            );
            let parent = &resumed.successors[0].payload;
            assert_eq!(
                parent.store.nonce(address(0x101)).numeric().exact(),
                Some(U256::ONE)
            );
            assert_eq!(
                parent.store.read_balance(address(0x101)).numeric().exact(),
                Some(U256::from(100))
            );
            assert_eq!(
                parent.store.read_balance(destination).numeric().exact(),
                Some(U256::from(50))
            );
        }
    }
}

#[test]
fn create_reuses_expensive_offset_discovery_through_allocation_and_initcode_entry() {
    for op in [opcode::CREATE, opcode::CREATE2] {
        let mut fixture = fixture(if op == opcode::CREATE { "f000" } else { "f500" }, false);
        let destination = if op == opcode::CREATE {
            address(0x101).create(0)
        } else {
            address(0x101).create2_from_code(U256::from(9).to_be_bytes::<32>(), [0])
        };
        fixture
            .world
            .insert(destination, EvmAccount::empty())
            .unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        let args = [word(3), watched(1), word(1), word(9)];
        let mut costs = Vec::new();
        for fee in [313, 1_000_000] {
            let _receipt = RECEIPT.scope();
            RECEIPT.update(|receipt| receipt.fee = fee);
            let mut budget = WorkBudget::new(fixture.config.max_work);
            let result = create(&fixture, op, &args, &mut budget);
            assert!(result.frontiers.is_empty());
            assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
            let child = result
                .successors
                .iter()
                .find(|successor| successor.kind == MachineEdgeKind::Call)
                .unwrap();
            let parent = child.payload.call_stack.parent().unwrap();
            assert_eq!(parent.memory.len().numeric().exact(), Some(U256::from(32)));
            assert!(
                parent
                    .memory
                    .allocation_facts()
                    .any(|(offset, size)| offset.as_constant() == Some(U256::ONE)
                        && size.as_constant() == Some(U256::ONE))
            );
            assert_eq!(child.payload.active().key.address, destination);
            assert_eq!(
                child
                    .payload
                    .store
                    .read_balance(address(0x101))
                    .numeric()
                    .exact(),
                Some(U256::from(97))
            );
            assert_eq!(
                child
                    .payload
                    .store
                    .read_balance(destination)
                    .numeric()
                    .exact(),
                Some(U256::from(3))
            );
            costs.push(budget.used());
        }
        assert_eq!(
            costs[1] - costs[0],
            1_000_000 - 313,
            "one query's named fee, without a structural-fee allowance"
        );
    }
}

#[test]
fn create_old_msize_discovery_is_paid_once_and_stops_before_metadata_installation() {
    let fixture = fixture("f000", false);
    let offset = word(1).with_expression(ExprId::constant(U256::ONE));
    let size = word(1).with_expression(ExprId::constant(U256::ONE));
    let certified = fixture.input.active().memory.clone();
    let zero_proof = ExprId::constant(U256::ZERO);
    assert_eq!(certified.len().expression(), Some(&zero_proof));
    assert!(certified.default_byte().expression().is_none());
    let mut uncertified = certified.clone();
    let mut cleared = 0;
    // Model the same opaque zero MSIZE without the constructor's optional
    // certificate. Only forget that proof; keep N, source and byte state intact.
    // Setup queries run outside the counter scopes used for paid discovery.
    uncertified.update_values(&mut |value| {
        if value.expression() == Some(&zero_proof) {
            value.forget_expression();
            cleared += 1;
        }
    });
    assert_eq!(cleared, 1);
    assert!(uncertified.len().expression().is_none());
    let mut restored = uncertified.clone();
    restored.retain_length_literal(zero_proof.clone());
    assert_eq!(restored, certified);
    for stop in 0..5 {
        let mut result = empty_execution(fixture.input.clone());
        result.payload.active_mut().memory = uncertified.clone();
        let before = result.payload.active().memory.clone();
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.watch_word = Some(U256::ZERO);
            receipt.fee = if stop == 1 { usize::MAX } else { 1_000_000 };
            receipt.on_quote = stop == 3;
            receipt.on_query = stop == 4;
        });
        let mut budget = WorkBudget::new(if stop == 2 { 999_999 } else { usize::MAX });
        let token = analysis_control::Cancellation::new();
        let answer = analysis_control::with_cancellation(&token, || {
            transfer::create::expand_prepared(
                &mut result,
                &offset,
                &size,
                &mut context(&fixture, &mut budget),
            )
        });
        assert_eq!(RECEIPT.get().quotes, 1);
        assert_eq!(RECEIPT.get().queries, usize::from(stop == 0 || stop == 4));
        if stop == 0 {
            assert_eq!(answer, Ok(()));
            assert!(!budget.exhausted());
            assert!(budget.used() > 1_000_000);
            assert_eq!(
                result.payload.active().memory.len().numeric().exact(),
                Some(U256::from(32))
            );
        } else {
            assert_eq!(
                answer,
                Err(if stop < 3 {
                    FrontierReason::Work
                } else {
                    FrontierReason::Relations(QueryReason::Cancelled)
                })
            );
            assert_eq!(result.payload.active().memory, before);
            assert_eq!(budget.used(), if stop == 4 { 1_000_000 } else { 0 });
        }
    }
    // The unchanged constructor proof also covers nonempty allocation. Even
    // a refusing/cancelling opaque query must be unnecessary on this path.
    {
        let mut result = empty_execution(fixture.input.clone());
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.watch_word = Some(U256::ZERO);
            receipt.fee = usize::MAX;
            receipt.on_quote = true;
            receipt.on_query = true;
        });
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(fixture.config.max_work);
        analysis_control::with_cancellation(&token, || {
            transfer::create::expand_prepared(
                &mut result,
                &offset,
                &size,
                &mut context(&fixture, &mut budget),
            )
        })
        .unwrap();
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
        assert!(!token.is_cancelled());
        assert!(!budget.exhausted());
        assert_eq!(
            result.payload.active().memory.len().numeric().exact(),
            Some(U256::from(32))
        );
    }
    // Empty creation cannot change MSIZE, even when its discovery is refused.
    let mut result = empty_execution(fixture.input.clone());
    result.payload.active_mut().memory = uncertified.clone();
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watch_word = Some(U256::ZERO);
        receipt.fee = usize::MAX;
    });
    let zero = word(0).with_expression(ExprId::constant(U256::ZERO));
    transfer::create::expand_prepared(
        &mut result,
        &offset,
        &zero,
        &mut context(&fixture, &mut WorkBudget::new(fixture.config.max_work)),
    )
    .unwrap();
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    assert_eq!(result.payload.active().memory, uncertified);
}

#[test]
fn create_allocation_reuses_negative_offset_discovery_without_changing_its_carrier() {
    let fixture = fixture("f000", true);
    let offset = EvmValue::from_numeric(Opaque {
        watched: true,
        low: U256::ONE,
        high: U256::from(2),
        finite: Some([U256::ONE, U256::from(2)].into()),
        ..Opaque::top()
    })
    .with_expression(ExprId::fresh().unwrap());
    let original = offset.clone();
    let size = word(1).with_expression(ExprId::constant(U256::ONE));
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = 1_000_000);
    let mut budget = WorkBudget::new(fixture.config.max_work);
    let answer = transfer::query::singleton(offset.numeric(), &mut budget).unwrap();
    assert_eq!(answer, None);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    let mut result = empty_execution(fixture.input.clone());
    assert!(transfer::ranges::memory_after_queries(
        &mut result,
        &offset,
        &size,
        SingletonProof::Answer(answer),
        SingletonProof::Answer(Some(U256::ONE)),
        &mut context(&fixture, &mut budget),
        0,
    ));
    assert!(result.frontiers.is_empty());
    // The complete numeric guard operations own their expression preparation.
    // Allocation must add neither a raw query nor a new standalone payment.
    let before = RECEIPT.get();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    transfer::create::expand_prepared(
        &mut result,
        &offset,
        &size,
        &mut context(&fixture, &mut budget),
    )
    .unwrap();
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries),
        (before.quotes, before.queries)
    );
    assert_eq!(offset, original);
    assert!(offset.numeric().exact().is_none());
    assert!(
        result
            .payload
            .active()
            .memory
            .len()
            .numeric()
            .contains(U256::from(32))
    );
    assert!(
        result
            .payload
            .active()
            .memory
            .allocation_facts()
            .any(|(at, length)| Some(at) == offset.expression()
                && length.as_constant() == Some(U256::ONE))
    );
}

#[derive(Clone, Copy, Debug)]
enum JumpCover {
    Open,
    Valid,
    InvalidThenValid,
}

#[test]
fn jump_visit_refusal_keeps_exactly_the_completed_edges_and_faults() {
    for conditional in [false, true] {
        for cover in [
            JumpCover::Open,
            JumpCover::Valid,
            JumpCover::InvalidThenValid,
        ] {
            let mut fixture = fixture(
                if conditional {
                    "575b005b00"
                } else {
                    "565b005b00"
                },
                false,
            );
            let invalid = matches!(cover, JumpCover::InvalidThenValid);
            let target = EvmValue::from_numeric(Opaque {
                low: U256::ZERO,
                high: U256::from(3),
                finite: match cover {
                    JumpCover::Open => None,
                    JumpCover::Valid => Some([U256::ONE, U256::from(3)].into()),
                    JumpCover::InvalidThenValid => Some([U256::ZERO, U256::from(3)].into()),
                },
                ..Opaque::top()
            });
            fixture.input.active_mut().stack = if conditional {
                vec![word(1), target]
            } else {
                vec![target]
            };
            let label = format!("conditional {conditional}, cover {cover:?}");
            let roots = usize::from(!matches!(cover, JumpCover::Valid));
            let mut full_budget = WorkBudget::new(fixture.config.max_work);
            let full = run(&fixture, &mut full_budget);
            assert!(full.frontiers.is_empty(), "{label}");
            assert_eq!(
                full.successors.len(),
                if invalid { 1 } else { 2 },
                "{label}"
            );
            assert_eq!(full.outcomes.len(), roots, "{label}");

            // Refuse only the last target visit. Valid and invalid completed
            // candidates have different evidence, all checked at this boundary.
            let mut budget = WorkBudget::new(full_budget.used() - 1);
            let result = run(&fixture, &mut budget);
            assert!(budget.exhausted(), "{label}");
            assert_eq!(budget.used(), full_budget.used() - 1, "{label}");
            assert_stop_frontier(&result, FrontierReason::Work, &label);
            assert_eq!(result.frontiers[0].pc, 0, "{label}");
            assert_eq!(result.successors.len(), usize::from(!invalid), "{label}");
            if !invalid {
                assert_eq!(
                    result.successors[0].kind,
                    MachineEdgeKind::Intraprocedural(if conditional {
                        EdgeKind::BranchTrue
                    } else {
                        EdgeKind::Jump
                    }),
                    "{label}"
                );
                assert_eq!(
                    result.successors[0].payload.active().key.basic_block_index,
                    fixture
                        .input
                        .active()
                        .program
                        .as_ref()
                        .unwrap()
                        .jumpdest_blocks()[&1],
                    "{label}"
                );
            } else {
                assert_eq!(
                    result.diagnostics,
                    [(0, crate::analysis::DiagnosticKind::InvalidJump)],
                    "{label}"
                );
            }
            assert_eq!(result.outcomes.len(), roots, "{label}");
            for outcome in result.outcomes {
                assert_eq!(outcome.kind, OutcomeKind::Failure, "{label}");
                assert_eq!(outcome.data, EvmByteArray::empty(), "{label}");
                assert_eq!(
                    outcome.store,
                    *fixture.input.active().saved_store.state(),
                    "{label}"
                );
            }
        }
    }
}

#[test]
fn create2_salt_refusal_and_late_collision_cancellation_leave_no_partial_creation() {
    for point in 0..3 {
        let mut fixture = fixture("f500", false);
        let destination = address(0x101).create2_from_code(U256::from(9).to_be_bytes::<32>(), []);
        let mut account = EvmAccount::empty();
        account.nonce = word(0);
        account.balance = word(0);
        fixture.world.insert(destination, account).unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        let mut args = vec![word(0), word(0), word(0), watched(9)];
        if point == 2 {
            args[3] = word(9);
            fixture.input.store.write_nonce(destination, watched(0));
        }
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.fee = if point == 0 { usize::MAX } else { 313 };
            receipt.on_quote = point == 1;
            receipt.on_query = point == 2;
        });
        let active = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(fixture.config.max_work);
        let result = analysis_control::with_cancellation(&active, || {
            create(&fixture, opcode::CREATE2, &args, &mut budget)
        });
        assert_stopped_after_failures(
            &result,
            if point == 0 {
                FrontierReason::Work
            } else {
                FrontierReason::Relations(QueryReason::Cancelled)
            },
            &fixture,
            if point == 2 {
                PublishedBeforeStop::RootFaultAndCreationRejection
            } else {
                PublishedBeforeStop::RootFault
            },
            &format!("CREATE2 salt/collision {point}"),
        );
        assert_eq!(result.payload.store, fixture.input.store);
        assert_eq!(RECEIPT.get().queries, usize::from(point == 2));
    }
}

#[test]
fn runtime_extent_is_paid_once_and_reused_during_exact_deployment() {
    let mut fixture = fixture("f000", false);
    fixture
        .world
        .insert(address(0x101).create(0), EvmAccount::absent())
        .unwrap();
    fixture.input =
        transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
    let args = [word(0), word(0), word(0)];
    let result = create(
        &fixture,
        opcode::CREATE,
        &args,
        &mut WorkBudget::new(fixture.config.max_work),
    );
    let child = result
        .successors
        .into_iter()
        .find(|successor| successor.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    let address = child.active().key.address;
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.watch_word = Some(U256::from(7)));
    let mut payload = child.clone();
    let mut kind = OutcomeKind::Return;
    let mut data = EvmByteArray::exact(&[0; 7]);
    transfer::create::finish_creation(
        &mut payload,
        &mut kind,
        &mut data,
        &mut context(&fixture, &mut WorkBudget::new(fixture.config.max_work)),
        1,
    )
    .unwrap();
    assert_eq!(kind, OutcomeKind::Return);
    assert_eq!(payload.store.raw_account_code(address), Some(vec![0; 7]));
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    for point in 0..4 {
        RECEIPT.update(|receipt| {
            receipt.quotes = 0;
            receipt.queries = 0;
            receipt.fee = if point == 0 { usize::MAX } else { 313 };
            receipt.on_quote = point == 2;
            receipt.on_query = point == 3;
        });
        let mut payload = child.clone();
        let mut kind = OutcomeKind::Return;
        let mut data = EvmByteArray::exact(&[0; 7]);
        let before = data.clone();
        let mut budget = WorkBudget::new(if point == 1 { 312 } else { usize::MAX });
        let token = analysis_control::Cancellation::new();
        let error = analysis_control::with_cancellation(&token, || {
            transfer::create::finish_creation(
                &mut payload,
                &mut kind,
                &mut data,
                &mut context(&fixture, &mut budget),
                1,
            )
        })
        .unwrap_err();
        assert_eq!(
            error,
            if point >= 2 {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            }
        );
        assert_eq!(payload.store, child.store);
        assert_eq!(kind, OutcomeKind::Return);
        assert_eq!(data, before);
        assert_eq!(RECEIPT.get().queries, usize::from(point == 3));
        assert_eq!(budget.used(), if point == 3 { 313 } else { 0 });
    }
}

#[test]
fn every_creation_candidate_refuses_before_discovery_and_keeps_the_paid_prefix() {
    for site in 0..9 {
        let mut fixture = fixture("f500", false);
        let initcode: &[u8] = if site == 1 { &[0] } else { &[] };
        let destination =
            address(0x101).create2_from_code(U256::from(9).to_be_bytes::<32>(), initcode);
        let mut account = EvmAccount::empty();
        account.nonce = word(0);
        account.balance = word(50);
        fixture.world.insert(destination, account).unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        let mut args = vec![word(3), word(0), word(0), word(9)];
        match site {
            0 => args[2] = watched(0),
            1 => {
                args[1] = watched(1);
                args[2] = word(1);
            }
            2 => fixture.input.active_mut().address_operand = Some(watched(0x101)),
            3 => fixture.input.store.write_nonce(address(0x101), watched(0)),
            4 => args[0] = watched(3),
            5 => args[3] = watched(9),
            6 => fixture
                .input
                .store
                .write_balance(address(0x101), watched(100)),
            7 => fixture.input.store.write_nonce(destination, watched(0)),
            8 => fixture.input.store.write_balance(destination, watched(50)),
            _ => unreachable!(),
        }
        assert_query_stops(
            &fixture,
            QueryStopCase::new(
                &format!("CREATE2 candidate {site}"),
                match site {
                    0 | 1 => PublishedBeforeStop::Nothing,
                    2..=6 => PublishedBeforeStop::RootFault,
                    7 | 8 => PublishedBeforeStop::RootFaultAndCreationRejection,
                    _ => unreachable!(),
                },
            ),
            |budget| create(&fixture, opcode::CREATE2, &args, budget),
        );
    }
}

fn runtime_prefix_fixture(
    cap: usize,
    owner: Option<analysis_control::Cancellation>,
) -> (
    Fixture,
    crate::analysis::EvmMachinePayload<Opaque>,
    EvmByteArray<Opaque>,
) {
    let mut fixture = fixture("f000", false);
    fixture.config.max_memory_bytes = cap;
    fixture
        .world
        .insert(address(0x101).create(0), EvmAccount::absent())
        .unwrap();
    fixture.input =
        transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
    let result = create(
        &fixture,
        opcode::CREATE,
        &[word(0), word(0), word(0)],
        &mut WorkBudget::new(fixture.config.max_work),
    );
    let child = result
        .successors
        .into_iter()
        .find(|successor| successor.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    let domain = fixture.algebra.borrowed();
    let mut data = EvmByteArray::unknown()
        .slice(&word(0), &word(2), domain)
        .unwrap();
    let prefix = EvmValue::from_numeric(Opaque {
        watched: true,
        cancel_owner: owner,
        ..Opaque::constant(U256::from(0xef))
    })
    .with_expression(ExprId::fresh().unwrap());
    data.write_byte(&word(0), &prefix, domain).unwrap();
    assert!(data.exact_bytes_bounded(2, domain).is_none());
    (fixture, child, data)
}

#[test]
fn runtime_forbidden_prefix_rejects_unknown_suffix_even_above_extraction_cap() {
    for cap in [1, 2] {
        let (fixture, mut child, mut data) = runtime_prefix_fixture(cap, None);
        let before = child.clone();
        let _receipt = RECEIPT.scope();
        let mut budget = WorkBudget::new(fixture.config.max_work);
        let mut kind = OutcomeKind::Return;
        transfer::create::finish_creation(
            &mut child,
            &mut kind,
            &mut data,
            &mut context(&fixture, &mut budget),
            1,
        )
        .unwrap();
        assert_eq!(kind, OutcomeKind::Failure);
        assert_eq!(data, EvmByteArray::empty());
        assert_eq!(child, before, "prefix rejection cannot install any runtime");
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
        assert!(!budget.exhausted());
    }
}

#[test]
fn runtime_prefix_refusal_and_both_cancellation_owners_leave_deployment_unchanged() {
    for point in 0..6 {
        let token = analysis_control::Cancellation::new();
        let captured = point >= 4;
        let (fixture, child, original) = runtime_prefix_fixture(1, captured.then(|| token.clone()));
        let _receipt = RECEIPT.scope();
        let mut complete = WorkBudget::new(fixture.config.max_work);
        transfer::create::finish_creation(
            &mut child.clone(),
            &mut OutcomeKind::Return,
            &mut original.clone(),
            &mut context(&fixture, &mut complete),
            1,
        )
        .unwrap();
        // Completion also pays for the rejection's empty buffer. Measure the
        // prefix at the refused byte query so that suffix cannot fund this seam.
        RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        let mut refused = WorkBudget::new(fixture.config.max_work);
        assert_eq!(
            transfer::create::finish_creation(
                &mut child.clone(),
                &mut OutcomeKind::Return,
                &mut original.clone(),
                &mut context(&fixture, &mut refused),
                1,
            ),
            Err(FrontierReason::Work)
        );
        let prefix = refused.used();
        assert!(complete.used() > prefix + 313);
        RECEIPT.update(|receipt| {
            receipt.quotes = 0;
            receipt.queries = 0;
            receipt.fee = if point == 0 { usize::MAX } else { 313 };
            receipt.on_quote = point == 2 || point == 4;
            receipt.on_query = point == 3 || point == 5;
        });
        let limit = if point == 1 { prefix + 312 } else { usize::MAX };
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(limit))
        } else {
            WorkBudget::new(limit)
        };
        let active = if captured {
            analysis_control::Cancellation::new()
        } else {
            token.clone()
        };
        let mut payload = child.clone();
        let mut data = original.clone();
        let mut kind = OutcomeKind::Return;
        let error = analysis_control::with_cancellation(&active, || {
            transfer::create::finish_creation(
                &mut payload,
                &mut kind,
                &mut data,
                &mut context(&fixture, &mut budget),
                1,
            )
        })
        .unwrap_err();
        assert_eq!(
            error,
            if point < 2 {
                FrontierReason::Work
            } else {
                FrontierReason::Relations(QueryReason::Cancelled)
            }
        );
        assert_eq!(payload, child);
        assert_eq!(kind, OutcomeKind::Return);
        assert_eq!(data, original);
        assert_eq!(RECEIPT.get().queries, usize::from(point == 3 || point == 5));
        assert_eq!(
            budget.used(),
            prefix + if point == 3 || point == 5 { 313 } else { 0 }
        );
    }
}

#[test]
fn call_target_balance_and_selfdestruct_queries_refuse_without_published_effects() {
    for site in 0..4 {
        let mut fixture = fixture(if site < 2 { "f100" } else { "ff" }, false);
        let mut args = call_args(word(3), word(0x202));
        match site {
            0 => args[1] = watched(0x202),
            1 => fixture
                .input
                .store
                .write_balance(address(0x101), watched(100)),
            2 => fixture.input.active_mut().stack = vec![watched(0x202)],
            3 => {
                fixture.input.active_mut().stack = vec![word(0x202)];
                fixture
                    .input
                    .store
                    .write_balance(address(0x101), watched(100));
            }
            _ => unreachable!(),
        }
        let run_case = |budget: &mut WorkBudget| {
            if site < 2 {
                call(&fixture, &args, budget)
            } else {
                run(&fixture, budget)
            }
        };
        assert_query_stops(
            &fixture,
            QueryStopCase::new(
                &format!("CALL/SELFDESTRUCT operand {site}"),
                PublishedBeforeStop::Nothing,
            ),
            run_case,
        );
    }
}

#[test]
fn static_call_zero_and_nonzero_conditions_keep_real_fault_and_entry_paths() {
    for amount in [0, 3] {
        let mut fixture = fixture("f100", false);
        fixture.input.active_mut().key.is_static = true;
        let _receipt = RECEIPT.scope();
        let result = call(
            &fixture,
            &call_args(watched(amount), word(0x202)),
            &mut WorkBudget::new(fixture.config.max_work),
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(
            result
                .successors
                .iter()
                .any(|successor| successor.kind == MachineEdgeKind::Call),
            amount == 0
        );
        // A zero CALL also retains the ordinary abstract gas-failure envelope.
        if amount != 0 {
            assert!(
                result
                    .outcomes
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Failure)
            );
        }
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
        for successor in result
            .successors
            .iter()
            .filter(|successor| successor.kind == MachineEdgeKind::Call)
        {
            assert_eq!(
                successor.payload.store.read_balance(address(0x101)),
                fixture.input.store.read_balance(address(0x101))
            );
            assert_eq!(
                successor.payload.store.read_balance(address(0x202)),
                fixture.input.store.read_balance(address(0x202))
            );
            assert!(successor.payload.active().key.is_static);
        }
    }
}

#[test]
fn selfdestruct_zero_can_keep_a_beneficiary_absent_and_uncertain_balance_is_only_may_present() {
    for amount in 0..3 {
        let mut fixture = fixture("ff", false);
        fixture
            .world
            .insert(address(0x404), EvmAccount::absent())
            .unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        let balance = if amount < 2 {
            word(if amount == 0 { 0 } else { 3 })
        } else {
            EvmValue::from_numeric(Opaque {
                low: U256::ZERO,
                high: U256::from(3),
                ..Opaque::top()
            })
        };
        fixture.input.store.write_balance(address(0x101), balance);
        fixture.input.active_mut().stack = vec![watched(0x404)];
        let _receipt = RECEIPT.scope();
        let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
        assert!(result.frontiers.is_empty());
        let returned = result
            .outcomes
            .iter()
            .find(|outcome| outcome.kind == OutcomeKind::Return)
            .unwrap();
        assert_eq!(
            returned.store.existence(address(0x404)),
            [Existence::Absent, Existence::Present, Existence::Unknown][amount]
        );
        assert_eq!(
            returned.store.touched_in_transaction(address(0x404)),
            Some(true)
        );
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    }
}

#[test]
fn extcodehash_pays_empty_account_classification_and_keeps_zero_nonempty_hash_witnesses() {
    let empty_hash = U256::from_be_slice(alloy_primitives::keccak256([]).as_slice());
    for nonce in [false, true] {
        for amount in [0, 7] {
            let mut fixture = fixture("3f00", false);
            fixture.input.active_mut().stack = vec![word(0x202)];
            fixture.input.store.write_balance(
                address(0x202),
                if nonce { word(0) } else { watched(amount) },
            );
            fixture.input.store.write_nonce(
                address(0x202),
                if nonce { watched(amount) } else { word(0) },
            );
            let before = fixture.input.store.clone();
            let _receipt = RECEIPT.scope();
            let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
            assert!(result.frontiers.is_empty());
            assert_eq!(
                result
                    .payload
                    .active()
                    .stack
                    .last()
                    .unwrap()
                    .numeric()
                    .exact(),
                Some(if amount == 0 { U256::ZERO } else { empty_hash })
            );
            assert_eq!(result.payload.store, before);
            assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
        }
    }
    for exclude_zero in [false, true] {
        let mut fixture = fixture("3f00", false);
        fixture.input.active_mut().stack = vec![word(0x202)];
        fixture.input.store.write_balance(
            address(0x202),
            EvmValue::from_numeric(Opaque {
                watched: true,
                low: U256::ZERO,
                high: U256::from(7),
                excluded: exclude_zero.then_some(U256::ZERO),
                ..Opaque::top()
            }),
        );
        let _receipt = RECEIPT.scope();
        let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
        assert!(result.frontiers.is_empty());
        let value = result.payload.active().stack.last().unwrap().numeric();
        assert_eq!(value.high, empty_hash);
        assert_eq!(
            value.low,
            if exclude_zero { empty_hash } else { U256::ZERO }
        );
        assert_eq!(
            (
                RECEIPT.get().quotes,
                RECEIPT.get().queries,
                RECEIPT.get().membership
            ),
            (1, 1, 1)
        );
    }
}

#[test]
fn extcodehash_known_absence_or_nonempty_code_needs_no_balance_query() {
    for absent in [false, true] {
        let mut fixture = fixture("3f00", false);
        fixture
            .world
            .insert(
                address(0x404),
                if absent {
                    EvmAccount::absent()
                } else {
                    EvmAccount::from_hex("00", Fork::Osaka).unwrap()
                },
            )
            .unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        fixture.input.active_mut().stack = vec![word(0x404)];
        fixture
            .input
            .store
            .write_balance(address(0x404), watched(0));
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        let result = run(&fixture, &mut WorkBudget::new(fixture.config.max_work));
        assert!(result.frontiers.is_empty());
        assert_eq!(
            result
                .payload
                .active()
                .stack
                .last()
                .unwrap()
                .numeric()
                .exact(),
            Some(if absent {
                U256::ZERO
            } else {
                U256::from_be_slice(alloy_primitives::keccak256([0]).as_slice())
            })
        );
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    }
}

#[test]
fn extcodehash_max_short_and_cancelling_account_queries_publish_no_hash_or_terminal() {
    for nonce in [false, true] {
        let mut fixture = fixture("3f00", false);
        fixture.input.active_mut().stack = vec![word(0x202)];
        fixture
            .input
            .store
            .write_balance(address(0x202), if nonce { word(0) } else { watched(0) });
        fixture
            .input
            .store
            .write_nonce(address(0x202), if nonce { watched(0) } else { word(0) });
        assert_query_stops(
            &fixture,
            QueryStopCase {
                empty_stack: true,
                ..QueryStopCase::new(
                    if nonce {
                        "EXTCODEHASH nonce"
                    } else {
                        "EXTCODEHASH balance"
                    },
                    PublishedBeforeStop::Nothing,
                )
            },
            |budget| run(&fixture, budget),
        );
    }
}

#[test]
fn changed_call_and_selfdestruct_balances_need_their_own_result_proofs() {
    for call_case in [false, true] {
        for relations in [false, true] {
            let mut fixture = fixture(if call_case { "f100" } else { "ff" }, relations);
            fixture.input.active_mut().stack = vec![word(0x202)];
            let args = call_args(word(3), word(0x202));
            let _receipt = RECEIPT.scope();
            RECEIPT.update(|receipt| receipt.watch_arithmetic = true);
            let mut budget = WorkBudget::new(fixture.config.max_work);
            let result = if call_case {
                call(&fixture, &args, &mut budget)
            } else {
                run(&fixture, &mut budget)
            };
            assert!(result.frontiers.is_empty());
            let results = if call_case { 2 } else { 1 };
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (if relations { 0 } else { results }, results)
            );
            let store = if call_case {
                &result
                    .successors
                    .iter()
                    .find(|successor| successor.kind == MachineEdgeKind::Call)
                    .unwrap()
                    .payload
                    .store
            } else {
                &result
                    .outcomes
                    .iter()
                    .find(|outcome| outcome.kind == OutcomeKind::Return)
                    .unwrap()
                    .store
            };
            assert_eq!(
                store.read_balance(address(0x101)).numeric().exact(),
                Some(U256::from(if call_case { 97 } else { 0 }))
            );
            assert_eq!(
                store.read_balance(address(0x202)).numeric().exact(),
                Some(U256::from(if call_case { 53 } else { 150 }))
            );
        }
    }
}

#[test]
fn refused_arithmetic_result_classification_installs_no_partial_balance_effects() {
    for call_case in [false, true] {
        let mut fixture = fixture(if call_case { "f100" } else { "ff" }, false);
        fixture.input.active_mut().stack = vec![word(0x202)];
        let args = call_args(word(3), word(0x202));
        let run_case = |budget: &mut WorkBudget| {
            if call_case {
                call(&fixture, &args, budget)
            } else {
                run(&fixture, budget)
            }
        };
        assert_query_stops(
            &fixture,
            QueryStopCase {
                watch_arithmetic: true,
                ..QueryStopCase::new(
                    if call_case {
                        "CALL debit result"
                    } else {
                        "SELFDESTRUCT credit result"
                    },
                    if call_case {
                        PublishedBeforeStop::RootFaultAndCallRejection
                    } else {
                        PublishedBeforeStop::Nothing
                    },
                )
            },
            run_case,
        );
    }
}

#[test]
fn static_mixed_value_keeps_its_completed_fault_when_the_zero_paths_target_stops() {
    for relations in [false, true] {
        let mut fixture = fixture("f100", relations);
        fixture.input.active_mut().key.is_static = true;
        let value = EvmValue::from_numeric(Opaque {
            low: U256::ZERO,
            high: U256::ONE,
            ..Opaque::top()
        })
        .with_expression(ExprId::fresh().unwrap());
        let args = call_args(value, watched(0x202));
        let before = fixture.input.store.clone();
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.fee = if relations { 313 } else { usize::MAX };
            // With relations enabled the complete projection owns discovery.
            receipt.on_query = relations;
        });
        let token = analysis_control::Cancellation::new();
        let result = analysis_control::with_cancellation(&token, || {
            call(&fixture, &args, &mut WorkBudget::new(usize::MAX))
        });
        assert_stopped_after_failures(
            &result,
            if relations {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            },
            &fixture,
            PublishedBeforeStop::RootFault,
            &format!("static mixed value, relations {relations}"),
        );
        assert_eq!(result.payload.store, before);
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries),
            if relations { (0, 1) } else { (1, 0) }
        );
        if relations {
            assert!(
                !result.payload.relations.is_empty(),
                "the zero-path guard is retained, while the nonzero fault was not erased"
            );
        }
    }
}

#[test]
fn creation_keeps_completed_oversize_and_nonce_gas_failures_on_later_query_stop() {
    for oversize in [false, true] {
        let mut fixture = fixture("f000", false);
        let destination = address(0x101).create(0);
        fixture
            .world
            .insert(destination, EvmAccount::absent())
            .unwrap();
        fixture.input =
            transfer::initial(&fixture.world, &fixture.entry, fixture.algebra.borrowed()).unwrap();
        let args = if oversize {
            vec![
                word(0),
                watched(1),
                EvmValue::from_numeric(Opaque {
                    low: U256::ONE,
                    high: U256::from(49_153),
                    finite: Some([U256::ONE, U256::from(49_153)].into()),
                    ..Opaque::top()
                }),
            ]
        } else {
            fixture.input.store.write_nonce(destination, watched(0));
            vec![word(0), word(0), word(0)]
        };
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        let result = create(
            &fixture,
            opcode::CREATE,
            &args,
            &mut WorkBudget::new(usize::MAX),
        );
        assert_stopped_after_failures(
            &result,
            FrontierReason::Work,
            &fixture,
            if oversize {
                PublishedBeforeStop::RootFault
            } else {
                PublishedBeforeStop::RootFaultAndCreationRejection
            },
            &format!("creation prior failure, oversize {oversize}"),
        );
        assert_eq!(RECEIPT.get().queries, 0);
    }
}

#[test]
fn call_projection_prepays_one_real_mask_factory_and_reuses_its_operands() {
    let fixture = fixture("f100", false);
    let args = call_args(word(0), word(0x202));
    let _receipt = RECEIPT.scope();
    let mut complete = WorkBudget::new(fixture.config.max_work);
    assert!(call(&fixture, &args, &mut complete).frontiers.is_empty());
    assert_eq!(
        (RECEIPT.get().mask_quotes, RECEIPT.get().mask_factories),
        (1, 1)
    );
    RECEIPT.update(|receipt| {
        receipt.mask_fee = 164;
        receipt.mask_quotes = 0;
        receipt.mask_factories = 0;
    });
    let mut increased = WorkBudget::new(fixture.config.max_work);
    assert!(call(&fixture, &args, &mut increased).frontiers.is_empty());
    assert_eq!(
        increased.used(),
        complete.used() + 100,
        "the one actual factory has exactly one named owner"
    );
    assert_eq!(
        (RECEIPT.get().mask_quotes, RECEIPT.get().mask_factories),
        (1, 1)
    );
    RECEIPT.update(|receipt| {
        receipt.mask_fee = usize::MAX;
        receipt.mask_factories = 0;
    });
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_stopped(&call(&fixture, &args, &mut unlimited), FrontierReason::Work);
    assert_eq!(RECEIPT.get().mask_factories, 0);
    let prefix = unlimited.used();
    RECEIPT.update(|receipt| receipt.mask_fee = 64);
    let mut short = WorkBudget::new(prefix + args[1].clone_work() + 63);
    assert_stopped(&call(&fixture, &args, &mut short), FrontierReason::Work);
    assert_eq!((RECEIPT.get().mask_factories, short.used()), (0, prefix));
    for quote in [true, false] {
        RECEIPT.update(|receipt| {
            receipt.mask_quote_cancel = quote;
            receipt.mask_factory_cancel = !quote;
            receipt.mask_factories = 0;
        });
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(usize::MAX);
        let result =
            analysis_control::with_cancellation(&token, || call(&fixture, &args, &mut budget));
        assert_stopped(&result, FrontierReason::Relations(QueryReason::Cancelled));
        assert_eq!(result.payload.store, fixture.input.store);
        assert_eq!(RECEIPT.get().mask_factories, usize::from(!quote));
        assert_eq!(
            budget.used(),
            prefix + if quote { 0 } else { args[1].clone_work() + 64 }
        );
    }
}
