//! Opaque carrier and receipts independent of the default product representation.

use super::*;
use crate::{
    Fork,
    analysis::{
        EnginePolicies, ExecutionConfig, Status, analyze_world_with,
        transfer::{TransferContext, relations},
    },
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, ValueAlgebra, WordObservation,
        facts::{FactError, Symbol},
        symbolic::ExprId,
    },
    test_instrumentation::Counter,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy)]
pub(in crate::analysis::transfer) struct Receipt {
    pub watched: Option<U256>,
    pub fee: usize,
    pub quotes: usize,
    pub queries: usize,
    pub cancel_on_quote: bool,
    pub cancel_on_query: bool,
    pub cancel_on_constrain: bool,
    pub cancel_on_reduce: bool,
    pub literal_fee: usize,
    pub cancel_on_apply: bool,
    pub query_tag: Option<u64>,
}

pub(in crate::analysis::transfer) static RECEIPT: Counter<Receipt> = Counter::new(Receipt {
    watched: Some(U256::from_limbs([7, 0, 0, 0])),
    fee: 61,
    quotes: 0,
    queries: 0,
    cancel_on_quote: false,
    cancel_on_query: false,
    cancel_on_constrain: false,
    cancel_on_reduce: false,
    literal_fee: 64,
    cancel_on_apply: false,
    query_tag: None,
});

pub(in crate::analysis::transfer) static LITERALS: Counter<usize> = Counter::new(0);

/// A range with an unexported exclusion and no finite/bits/range trait views.
#[derive(Clone, Debug, Serialize)]
pub(in crate::analysis::transfer) struct Word {
    pub lower: U256,
    pub upper: U256,
    pub excluded: Option<U256>,
    #[serde(skip)]
    pub cancel_query_owner: Option<analysis_control::Cancellation>,
    #[serde(skip)]
    pub cancel_quote_owner: Option<analysis_control::Cancellation>,
    #[serde(skip)]
    pub query_tag: Option<u64>,
}

// Instrumentation does not change the represented concrete set.
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        (self.lower, self.upper, self.excluded) == (other.lower, other.upper, other.excluded)
    }
}
impl Eq for Word {}

impl Word {
    fn exact(&self) -> Option<U256> {
        (self.lower == self.upper && self.excluded != Some(self.lower)).then_some(self.lower)
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..={} except {:?}",
            self.lower, self.upper, self.excluded
        )
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self {
            lower: U256::ZERO,
            upper: U256::MAX,
            excluded: None,
            cancel_query_owner: None,
            cancel_quote_owner: None,
            query_tag: None,
        }
    }
    fn constant(word: U256) -> Self {
        LITERALS.update(|count| *count += 1);
        Self {
            lower: word,
            upper: word,
            ..Self::top()
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        let (lower, upper) = facts.interval().unsigned_bounds();
        Self {
            lower,
            upper,
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        // Explicit export loses the interior exclusion without querying Self.
        self.exact().map_or_else(
            || WordObservation::unsigned_range(self.lower, self.upper).unwrap(),
            WordObservation::constant,
        )
    }
    fn constrain(&self, facts: &WordObservation) -> Result<Self, FactError> {
        RECEIPT.update(|receipt| {
            if receipt.cancel_on_constrain {
                analysis_control::current_cancellation().cancel();
            }
        });
        let (lower, upper) = facts.interval().unsigned_bounds();
        let mut next = Self {
            lower: self.lower.max(lower),
            upper: self.upper.min(upper),
            excluded: self.excluded,
            cancel_query_owner: self.cancel_query_owner.clone(),
            cancel_quote_owner: self.cancel_quote_owner.clone(),
            query_tag: self.query_tag,
        };
        if let Some(values) = facts.constants() {
            let mut retained = values.iter().copied().filter(|word| self.contains(*word));
            let Some(first) = retained.next() else {
                return Err(FactError::Contradiction {
                    subject: Symbol::THIS,
                });
            };
            next.lower = next.lower.max(first);
            next.upper = next.upper.min(retained.next_back().unwrap_or(first));
        }
        if next.lower > next.upper || next.exact().is_none() && next.lower == next.upper {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        Ok(next)
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|receipt| {
            if receipt.watched == self.exact()
                && receipt
                    .query_tag
                    .is_none_or(|tag| self.query_tag == Some(tag))
            {
                receipt.queries += 1;
                if receipt.cancel_on_query {
                    analysis_control::current_cancellation().cancel();
                }
            }
        });
        if let Some(owner) = &self.cancel_query_owner {
            owner.cancel();
        }
        self.exact()
    }
    fn singleton_work(&self) -> usize {
        let mut fee = 64;
        RECEIPT.update(|receipt| {
            if receipt.watched == self.exact()
                && receipt
                    .query_tag
                    .is_none_or(|tag| self.query_tag == Some(tag))
            {
                receipt.quotes += 1;
                fee = receipt.fee;
                if receipt.cancel_on_quote {
                    analysis_control::current_cancellation().cancel();
                }
            }
        });
        if let Some(owner) = &self.cancel_quote_owner {
            owner.cancel();
        }
        fee
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        64
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct Policy;

impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        crate::domain::Domain::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, a: &Word, b: &Word) -> Word {
        Word {
            lower: a.lower.min(b.lower),
            upper: a.upper.max(b.upper),
            excluded: a.excluded.filter(|word| !b.contains(*word)),
            ..Word::top()
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        Word {
            lower: if next.lower < old.lower {
                U256::ZERO
            } else {
                old.lower
            },
            upper: if next.upper > old.upper {
                U256::MAX
            } else {
                old.upper
            },
            excluded: old.excluded.filter(|word| !next.contains(*word)),
            ..Word::top()
        }
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.widen_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        RECEIPT.update(|receipt| {
            if receipt.cancel_on_reduce {
                analysis_control::current_cancellation().cancel();
            }
        });
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, _op: u8, _args: &[Word]) -> NumericReduction<Word> {
        RECEIPT.update(|receipt| {
            if receipt.cancel_on_apply {
                analysis_control::current_cancellation().cancel();
            }
        });
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        // This test policy's whole-operation envelope covers its own internals;
        // it supplies no credit for the transfer's separate result queries.
        args.fold(1024usize, |work, word| {
            work.saturating_add(word.work_size() * 8)
        })
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        let mut fee = 64;
        RECEIPT.update(|receipt| fee = receipt.literal_fee);
        fee
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        value
            .work_size()
            .saturating_add(facts.saturating_add(1).saturating_mul(1024))
    }
}

pub(in crate::analysis::transfer) fn context<'a>(
    config: &'a ExecutionConfig,
    budget: &'a mut WorkBudget,
    algebra: crate::domain::AlgebraRef<'a, Policy>,
) -> TransferContext<'a, Policy> {
    TransferContext {
        config,
        domain: algebra,
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

#[test]
fn independent_singleton_query_pays_once_and_refuses_before_execution() {
    let _receipt = RECEIPT.scope();
    let word = Word::constant(U256::from(7));
    let mut budget = WorkBudget::new(61);
    assert_eq!(singleton(&word, &mut budget), Ok(Some(U256::from(7))));
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (1, 1, 61)
    );
    let mut limited = WorkBudget::new(60);
    assert_eq!(singleton(&word, &mut limited), Err(FrontierReason::Work));
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, limited.used()),
        (2, 1, 0)
    );
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(singleton(&word, &mut unlimited), Err(FrontierReason::Work));
    assert_eq!(
        (
            RECEIPT.get().quotes,
            RECEIPT.get().queries,
            unlimited.used()
        ),
        (3, 1, 0)
    );
}

#[test]
fn singleton_query_observes_both_current_and_captured_cancellation() {
    let _receipt = RECEIPT.scope();
    let word = Word::constant(U256::from(7));
    let token = analysis_control::Cancellation::new();
    let mut captured = analysis_control::with_cancellation(&token, || WorkBudget::new(1000));
    token.cancel();
    assert_eq!(
        singleton(&word, &mut captured),
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    );
    let mut outside = WorkBudget::new(1000);
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            singleton(&word, &mut outside),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    let during_quote = analysis_control::Cancellation::new();
    RECEIPT.update(|receipt| receipt.cancel_on_quote = true);
    analysis_control::with_cancellation(&during_quote, || {
        assert_eq!(
            singleton(&word, &mut outside),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, outside.used()),
        (1, 0, 0)
    );
}

#[test]
fn cancellation_during_singleton_keeps_payment_but_discards_the_answer() {
    let _receipt = RECEIPT.scope();
    for cancel_ledger in [false, true] {
        let owner = analysis_control::Cancellation::new();
        let active = analysis_control::Cancellation::new();
        let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(1000));
        let word = Word {
            cancel_query_owner: cancel_ledger.then(|| owner.clone()),
            ..Word::constant(U256::from(7))
        };
        RECEIPT.update(|receipt| receipt.cancel_on_query = !cancel_ledger);
        analysis_control::with_cancellation(&active, || {
            assert_eq!(
                singleton(&word, &mut budget),
                Err(FrontierReason::Relations(QueryReason::Cancelled))
            );
        });
        assert_eq!(budget.used(), 61);
        assert_eq!(owner.is_cancelled(), cancel_ledger);
        assert_eq!(active.is_cancelled(), !cancel_ledger);
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 2));
}

#[test]
fn refinement_prepays_opaque_singletons_and_leaves_refused_values_intact() {
    let _receipt = RECEIPT.scope();
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let expression = ExprId::fresh().unwrap();
    let mut relations = crate::domain::relational::RelationState::default();
    relations.add_equal(
        &expression,
        &ExprId::constant(U256::from(7)),
        &config.analysis.relations,
    );
    let before = EvmValue::constant(U256::from(7)).with_expression(expression);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(61);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (1, 1, 61)
    );
    let mut refused = WorkBudget::new(60);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut refused, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 1));
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut refused = WorkBudget::new(usize::MAX);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut refused, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, refused.used()),
        (3, 1, 0)
    );
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(1000);
        assert_eq!(
            relations::refine(
                &mut value,
                &relations,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(budget.used(), 61);
    });
    assert_eq!(value, before);
}

fn scalar_fixture() -> (
    ExecutionConfig,
    EvmValue<Word>,
    crate::domain::relational::RelationState,
) {
    let config = ExecutionConfig::default();
    let expression = ExprId::fresh().unwrap();
    let before = EvmValue::from_numeric(Word {
        lower: U256::ONE,
        upper: U256::from(9),
        excluded: Some(U256::from(5)),
        ..Word::top()
    })
    .with_expression(expression.clone())
    .with_symbolic_limit();
    let mut relations = crate::domain::relational::RelationState::default();
    relations.add_equal(
        &expression,
        &ExprId::constant(U256::from(7)),
        &config.analysis.relations,
    );
    (config, before, relations)
}

#[test]
fn refined_result_has_one_paid_query_and_preserves_owned_constraints() {
    let _receipt = RECEIPT.scope();
    let (config, before, relations) = scalar_fixture();
    let algebra = ValueAlgebra::new(Policy);
    let mut budget = WorkBudget::new(1_000_000);
    let mut value = before.clone();
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(true)
    );
    assert_eq!(
        (value.numeric().lower, value.numeric().upper),
        (U256::from(7), U256::from(7))
    );
    assert_eq!(value.numeric().excluded, Some(U256::from(5)));
    assert!(
        value.contains(U256::from(7))
            && !value.contains(U256::from(5))
            && !value.contains(U256::from(8))
    );
    assert_eq!(value.expression(), before.expression());
    assert!(!value.symbolic_limit_reached());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));

    // The initial nonsingleton query and the scalar envelope have already been
    // paid. Refusal at the result query cannot install that detached import.
    let success_work = budget.used();
    let mut limited = WorkBudget::new(success_work - 1);
    let mut value = before.clone();
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut limited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(limited.used(), success_work - 61);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 1));

    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut unlimited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));
}

#[test]
fn refinement_checks_cancellation_after_import_reduction_and_result_query() {
    let _receipt = RECEIPT.scope();
    let (config, before, relations) = scalar_fixture();
    let algebra = ValueAlgebra::new(Policy);
    for stage in 0..3 {
        RECEIPT.update(|receipt| {
            receipt.cancel_on_constrain = stage == 0;
            receipt.cancel_on_reduce = stage == 1;
            receipt.cancel_on_query = stage == 2;
        });
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(1_000_000);
            let mut value = before.clone();
            assert_eq!(
                relations::refine(
                    &mut value,
                    &relations,
                    &mut context(&config, &mut budget, algebra.borrowed())
                ),
                Err(FrontierReason::Relations(QueryReason::Cancelled))
            );
            assert_eq!(value, before);
            assert!(budget.used() > 64);
        });
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
}

#[test]
fn opaque_exclusion_can_refute_a_relational_equality() {
    let (config, before, relations) = scalar_fixture();
    let mut value = EvmValue::from_numeric(Word {
        excluded: Some(U256::from(7)),
        ..before.numeric().clone()
    })
    .with_expression(before.expression().unwrap().clone());
    let before = value.clone();
    let algebra = ValueAlgebra::new(Policy);
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(false)
    );
    assert_eq!(value, before);
    assert!(!value.contains(U256::from(7)));
}

#[test]
fn branch_discovery_pays_once_and_stops_before_returning_a_refused_successor() {
    use crate::analysis::transfer::{empty_execution, initial};

    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let world = EvmWorld::<Word>::new(Fork::Osaka, "query branch");
    let entry = EvmEntry::concrete(
        Address::ZERO,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let copy_work = input.work_size();
    let before_relations = input.relations.clone();
    let condition = EvmValue::constant(U256::from(7)).with_expression(ExprId::fresh().unwrap());
    let _receipt = RECEIPT.scope();
    let mut result = empty_execution(input.clone());
    let mut budget = WorkBudget::new(copy_work + 61);
    let successor = relations::branch(
        &mut result,
        &condition,
        true,
        &mut context(&config, &mut budget, algebra.borrowed()),
        17,
    )
    .unwrap();
    assert_eq!(successor.relations, before_relations);
    assert!(result.frontiers.is_empty());
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (1, 1, copy_work + 61)
    );
    for maximum in [copy_work + 60, usize::MAX] {
        if maximum == usize::MAX {
            RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        }
        let mut result = empty_execution(input.clone());
        let mut budget = WorkBudget::new(maximum);
        assert!(
            relations::branch(
                &mut result,
                &condition,
                true,
                &mut context(&config, &mut budget, algebra.borrowed()),
                17
            )
            .is_none()
        );
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert_eq!(result.payload.relations, before_relations);
        assert_eq!(budget.used(), copy_work);
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut result = empty_execution(input);
        let mut budget = WorkBudget::new(1_000_000);
        assert!(
            relations::branch(
                &mut result,
                &condition,
                true,
                &mut context(&config, &mut budget, algebra.borrowed()),
                17
            )
            .is_none()
        );
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::Cancelled)
        );
    });
}

#[test]
fn complete_operation_envelope_owns_internal_queries_without_a_second_payment() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let algebra = ValueAlgebra::new(Policy);
    let arg = EvmValue::<Word>::top()
        .with_expression(ExprId::fresh().unwrap())
        .with_symbolic_limit();
    let work = algebra.operation_work(std::slice::from_ref(&arg));
    let mut budget = WorkBudget::new(work);
    let result = algebra
        .apply_budgeted(revm_bytecode::opcode::ISZERO, &[arg], &mut budget)
        .unwrap();
    assert!(result.value.symbolic_limit_reached());
    assert_eq!(budget.used(), work);
    // One result discovery reused by both facade metadata decisions, plus one
    // operand discovery for expression construction, all inside the envelope.
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 2));
}

#[test]
fn storage_discovery_prepays_the_opaque_owner_query() {
    use crate::analysis::transfer::{StorageReadPolicy, execute, initial};

    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let owner_word = U256::from(0x7171);
    let owner = Address::from_word(owner_word.into());
    let mut world = EvmWorld::new(Fork::Osaka, "query discovery");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex("5f5400", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let mut input = initial(&world, &entry, algebra.borrowed()).unwrap();
    input.active_mut().address_operand = Some(EvmValue::constant(owner_word));
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.watched = Some(owner_word));
    for refused in [true, false] {
        RECEIPT.update(|receipt| {
            receipt.fee = if refused { usize::MAX } else { 61 };
        });
        let mut budget = WorkBudget::new(10_000_000);
        let result = execute(
            &world,
            &entry,
            input.clone(),
            &config,
            algebra.borrowed(),
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Discover,
        );
        assert_eq!(
            result
                .frontiers
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Work),
            refused
        );
        assert_eq!(RECEIPT.get().quotes, if refused { 1 } else { 2 });
        assert_eq!(RECEIPT.get().queries, usize::from(!refused));
    }
}

#[test]
fn public_pop_probe_needs_no_singleton_discovery_even_with_a_max_quote() {
    let _receipt = RECEIPT.scope();
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "singleton query receipt");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex("60075000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let config = ExecutionConfig {
        max_work: 10_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(Policy);
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let skipped = analyze_world_with(
        world.clone(),
        entry.clone(),
        config.clone(),
        &algebra,
        &EnginePolicies::default(),
    )
    .unwrap();
    assert_eq!(skipped.status(), Status::Converged);
    assert!(skipped.frontiers().is_empty());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.quotes = 0;
        receipt.queries = 0;
    });
    let paid =
        analyze_world_with(world, entry, config, &algebra, &EnginePolicies::default()).unwrap();
    assert_eq!(paid.status(), Status::Converged);
    assert!(paid.frontiers().is_empty());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    assert_eq!(skipped.work(), paid.work());
    assert!(
        paid.states()
            .iter()
            .any(|state| state.exit_stack.is_empty())
    );
}
