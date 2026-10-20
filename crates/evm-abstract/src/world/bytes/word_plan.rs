//! Paid, optional word metadata memoization for the two EVM load consumers.

use super::{EvmByteArray, RangeError, conditional, expr, limits, numeric, operands, reads};
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::{AbstractValue, Domain};
use crate::domain::{EvmNumericAlgebra, WordElement};
#[cfg(test)]
use crate::world::ByteArray;
use crate::{
    domain::{
        EvmValue, ValueAlgebra,
        symbolic::{
            ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, SlotStateId,
            SlotStateKind, StateBinding,
        },
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::collections::BTreeMap;

// This quantum bounds only the decision to use an optional cache. It is never
// an expression policy or a boundary of the complete word-read algorithm.
const PLANNING_QUANTUM: usize = 4096;

/// Non-Clone and demand-local: the same owned source, offset and entire domain
/// policy determine admission, payment and execution. No pointer-only owner key
/// or borrowed mutable ByteArray survives preparation.
pub(crate) struct Plan<
    'a,
    N: WordElement = crate::domain::NumericValue,
    D: EvmNumericAlgebra<Element = N> = crate::domain::Domain,
> {
    root: ByteStateId,
    length: Option<EvmValue<N>>,
    facts: BTreeMap<ExprId, EvmValue<N>>,
    conditional: conditional::ConditionalFacts<N>,
    offset: EvmValue<N>,
    policy: PlanPolicy<'a, D>,
    failure: Option<RangeError>,
    preparation: Option<conditional::Preparation>,
    fallback_work: usize,
}

// Generic plans retain the original borrowed D. Only the legacy Domain wrapper
// owns its small policy, keeping the source-compatible deferred read API.
enum PlanPolicy<'a, D: EvmNumericAlgebra> {
    Borrowed(AlgebraRef<'a, D>),
    Owned(ValueAlgebra<D>),
}
impl<D: EvmNumericAlgebra> PlanPolicy<'_, D> {
    fn view(&self) -> AlgebraRef<'_, D> {
        match self {
            Self::Borrowed(domain) => *domain,
            Self::Owned(domain) => domain.borrowed(),
        }
    }
}

fn charge(budget: &mut WorkBudget, work: usize) -> Result<bool, RangeError> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(ExprError::Cancelled.into());
    }
    Ok(budget.charge(work))
}

fn fact_work<N: WordElement>(facts: &BTreeMap<ExprId, EvmValue<N>>) -> usize {
    facts.iter().fold(0usize, |work, (key, value)| {
        work.saturating_add(key.work_size())
            .saturating_add(value.work_size())
    })
}

impl<N: WordElement> EvmByteArray<N> {
    /// The caller still pays its original byte_work. Only the selected word
    /// fee moves here. Planning, snapshot copies and the selected algorithm
    /// each have a reservation before their own work or allocations.
    fn prepare_word_read_policy<'a, D: EvmNumericAlgebra<Element = N>>(
        &self,
        offset: &EvmValue<N>,
        policy: PlanPolicy<'a, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Plan<'a, N, D>>, RangeError> {
        let domain = policy.view();
        let length_work = if self.memory {
            0
        } else {
            self.length.work_size()
        };
        // Cached expanded counts pay for stack growth, visits and the bounded
        // operand-size scan before any walk. Exact byte payloads are not read.
        let quantum = self
            .root
            .work_size()
            .saturating_add(offset.work_size())
            .saturating_add(length_work)
            .saturating_mul(4)
            .saturating_add(self.operands.len())
            .saturating_add(8)
            .min(PLANNING_QUANTUM);
        if !charge(budget, quantum.saturating_mul(8))? {
            return Ok(None);
        }
        let mut census = Census::new(quantum);
        let metadata = census
            .spend(self.operands.len())
            .then(|| fact_work(&self.operands));
        let preparation = if self.conditional.len() != 0
            && let Some(metadata_work) = metadata
            && census.fits(&self.root, offset, (!self.memory).then_some(&self.length))
        {
            Some(conditional::preparation(
                self.root.work_size(),
                offset,
                metadata_work,
                self.operands.len(),
                &self.conditional,
                domain,
            ))
        } else {
            None
        };
        let fallback_work = self.word_numeric_work_with(offset, domain);
        let full_work = preparation.as_ref().map_or(fallback_work, |_| {
            self.root
                .work_size()
                .saturating_mul(32)
                .saturating_add(256 * 32)
        });
        if !charge(budget, full_work)? {
            return Ok(None);
        }
        let metadata = match metadata {
            Some(work) => work,
            None => {
                // Admission has already been rejected and the original full
                // word fee paid. This scan pays for the original complete fact
                // snapshot, never for reconsidering optional admission.
                if !charge(
                    budget,
                    self.operands.len().saturating_mul(2).saturating_add(1),
                )? {
                    return Ok(None);
                }
                fact_work(&self.operands)
            }
        };
        let freeze_work = metadata
            .saturating_add(self.conditional.work_size())
            .saturating_add(offset.work_size())
            .saturating_add(length_work)
            .saturating_add(32)
            .saturating_mul(2);
        if !charge(budget, freeze_work)? {
            return Ok(None);
        }
        Ok(Some(Plan {
            root: self.root.clone(),
            length: (!self.memory).then(|| self.length.clone()),
            facts: self.operands.clone(),
            conditional: self.conditional.clone(),
            offset: offset.clone(),
            policy,
            failure: self.failure.clone(),
            preparation,
            fallback_work,
        }))
    }
}

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Plan<'a, N, D> {
    /// MLOAD executes this only after its original touch_memory. Expansion
    /// changes length/allocations, neither of which participates in a memory
    /// word read. The content/fact snapshot here is the exact prepared owner.
    /// Ok(None) is exclusively an unpaid Work boundary, never a completed read.
    pub(crate) fn read_word_budgeted(
        mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<EvmValue<N>>, RangeError> {
        let domain = self.policy.view();
        if let Some(error) = self.failure {
            return Err(error);
        }
        crate::domain::symbolic::state_check(self.root.nodes(), self.root.depth(), limits(domain))?;
        let offset = expr(&self.offset)?;
        operands::remember(
            &mut self.facts,
            offset.clone(),
            &self.offset,
            domain,
            limits(domain),
        )?;
        self.conditional.check(limits(domain))?;
        if let Some(preparation) = self.preparation {
            // The entire Context/Memo/Reader graph dies in this block, ending
            // the original budget borrow before a complete fallback can pay.
            let (value, work_exhausted) = {
                let Some(context) = conditional::Context::<N, D>::budgeted(
                    &self.facts,
                    &self.conditional,
                    domain,
                    &preparation,
                    budget,
                )?
                else {
                    return Ok(None);
                };
                let value = evaluate(&self.root, self.length.as_ref(), &offset, &context, domain)?;
                context.check()?;
                (value, context.work_exhausted())
            };
            if work_exhausted {
                return Ok(None);
            }
            if let Some(value) = value {
                return Ok(Some(value));
            }
            // Nothing from a refused attempt escapes. Pay the entire original
            // fee as additional work; planning/cache work is never refunded.
            if !charge(budget, self.fallback_work)? {
                return Ok(None);
            }
        }
        // The original full-read fee is already paid. Keep the same ledger
        // available for additional custom binding work, including after an
        // optional prepared-cache refusal, without charging old actions twice.
        let binding_budget = conditional::BindingBudget::new(budget);
        let context = conditional::Context::<N, D>::prepaid(
            &self.facts,
            Some(&self.conditional),
            domain,
            &binding_budget,
        );
        evaluate(&self.root, self.length.as_ref(), &offset, &context, domain)
    }
}

/// Shared complete symbolic/numeric algorithm. Only a private prepared Context
/// can return None for mandatory Work or optional refusal; true expression
/// limits and cancellation remain errors.
pub(super) fn evaluate<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &ByteStateId,
    length: Option<&EvmValue<N>>,
    offset: &ExprId,
    context: &conditional::Context<'a, N, D>,
    domain: AlgebraRef<'a, D>,
) -> Result<Option<EvmValue<N>>, RangeError> {
    context.check()?;
    if context.stopped() {
        return Ok(None);
    }
    let expression = expression(root, length, offset, context, domain);
    context.check()?;
    if context.stopped() {
        return Ok(None);
    }
    let expression = expression?;
    // A summary may retain an unsupported whole-word predicate for precisely
    // this normalized read. Different roots, padding and overlapping writes
    // produce different terms and cannot use the old guarantee.
    let retained = context.retained_word(&expression, domain);
    context.check()?;
    if context.stopped() {
        return Ok(None);
    }
    if let Some(value) = retained {
        return Ok(Some(numeric::attach(value, expression)));
    }
    let numeric = numeric::Reader::<N, D>::with_context(context.clone(), domain).word(root, offset);
    context.check()?;
    if context.stopped() {
        return Ok(None);
    }
    Ok(Some(numeric::attach(numeric, expression)))
}

pub(super) fn expression<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &ByteStateId,
    length: Option<&EvmValue<N>>,
    offset: &ExprId,
    context: &conditional::Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> Result<ExprId, RangeError> {
    let view = if let Some(length) = length {
        ByteView::new(
            root.clone(),
            ExprId::constant(U256::ZERO),
            length
                .symbolic_expression()
                .expect("every byte length has an expression or singleton"),
        )
        .slice(
            offset.clone(),
            ExprId::constant(U256::from(32)),
            limits(domain).into(),
        )?
    } else {
        ByteView::new(
            root.clone(),
            offset.clone(),
            ExprId::constant(U256::from(32)),
        )
    };
    Ok(reads::word_expression(
        view,
        ExprId::constant(U256::ZERO),
        context,
        domain,
        limits(domain),
    )?)
}

#[cfg(test)]
mod payer_tests;

// Bound the paid preflight scan by occurrences, including source views, scalar
// offsets and bindings even where the numeric evaluator treats a read as opaque.
// Admission allows an attempt; the shared runtime demand bounds actual contexts
// and metadata work, and refusal still requires the complete original word fee.
enum Node<'a> {
    Expr(&'a ExprId),
    Bytes(&'a ByteStateId),
    Slots(&'a SlotStateId),
    View(&'a ByteView),
    Bindings(&'a [StateBinding]),
}

struct Census<'a> {
    remaining: usize,
    pending: Vec<Node<'a>>,
}

impl<'a> Census<'a> {
    fn new(remaining: usize) -> Self {
        Self {
            remaining,
            pending: Vec::new(),
        }
    }

    fn spend(&mut self, amount: usize) -> bool {
        let Some(remaining) = self.remaining.checked_sub(amount) else {
            return false;
        };
        self.remaining = remaining;
        true
    }

    fn push(&mut self, node: Node<'a>) -> bool {
        if !self.spend(1) {
            return false;
        }
        self.pending.push(node);
        true
    }

    fn fits<N: WordElement>(
        &mut self,
        root: &'a ByteStateId,
        offset: &'a EvmValue<N>,
        length: Option<&'a EvmValue<N>>,
    ) -> bool {
        if !self.push(Node::Bytes(root)) {
            return false;
        }
        for value in [Some(offset), length].into_iter().flatten() {
            if let Some(expression) = value.expression()
                && !self.push(Node::Expr(expression))
            {
                return false;
            }
        }
        while let Some(node) = self.pending.pop() {
            if analysis_control::current_cancellation().is_cancelled() {
                return false;
            }
            let pushed = match node {
                Node::Expr(expression) => match expression.kind() {
                    ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_) => true,
                    ExprKind::Operation { args, .. } => {
                        args.iter().all(|arg| self.push(Node::Expr(arg)))
                    }
                    ExprKind::Ite {
                        condition,
                        then_value,
                        else_value,
                    } => {
                        self.push(Node::Expr(condition))
                            && self.push(Node::Expr(then_value))
                            && self.push(Node::Expr(else_value))
                    }
                    ExprKind::ByteRead { state, index } => {
                        self.push(Node::Bytes(state)) && self.push(Node::Expr(index))
                    }
                    ExprKind::WordRead { view, offset } => {
                        self.push(Node::View(view)) && self.push(Node::Expr(offset))
                    }
                    ExprKind::SlotRead { state, key } => {
                        self.push(Node::Slots(state)) && self.push(Node::Expr(key))
                    }
                    ExprKind::Keccak { input } => self.push(Node::View(input)),
                },
                Node::Bytes(state) => match state.kind() {
                    ByteStateKind::Zero | ByteStateKind::Exact(_) => true,
                    ByteStateKind::Input {
                        source,
                        length,
                        bindings,
                    } => {
                        self.push(Node::Expr(source.token()))
                            && self.push(Node::Expr(length))
                            && self.push(Node::Bindings(bindings))
                    }
                    ByteStateKind::WriteByte {
                        previous,
                        offset,
                        value,
                    }
                    | ByteStateKind::WriteWord {
                        previous,
                        offset,
                        value,
                    } => {
                        self.push(Node::Bytes(previous))
                            && self.push(Node::Expr(offset))
                            && self.push(Node::Expr(value))
                    }
                    ByteStateKind::Copy {
                        previous,
                        destination,
                        source,
                        length,
                    } => {
                        self.push(Node::Bytes(previous))
                            && self.push(Node::Expr(destination))
                            && self.push(Node::View(source))
                            && self.push(Node::Expr(length))
                    }
                    ByteStateKind::Slice { source } => self.push(Node::View(source)),
                    ByteStateKind::Ite {
                        condition,
                        then_state,
                        else_state,
                    } => {
                        self.push(Node::Expr(condition))
                            && self.push(Node::Bytes(then_state))
                            && self.push(Node::Bytes(else_state))
                    }
                },
                Node::Slots(state) => match state.kind() {
                    SlotStateKind::Zero => true,
                    SlotStateKind::Input { source, bindings } => {
                        self.push(Node::Expr(source.token())) && self.push(Node::Bindings(bindings))
                    }
                    SlotStateKind::Store {
                        previous,
                        key,
                        value,
                    } => {
                        self.push(Node::Slots(previous))
                            && self.push(Node::Expr(key))
                            && self.push(Node::Expr(value))
                    }
                    SlotStateKind::Ite {
                        condition,
                        then_state,
                        else_state,
                    } => {
                        self.push(Node::Expr(condition))
                            && self.push(Node::Slots(then_state))
                            && self.push(Node::Slots(else_state))
                    }
                },
                Node::View(view) => {
                    self.push(Node::Bytes(&view.state))
                        && self.push(Node::Expr(&view.offset))
                        && self.push(Node::Expr(&view.length))
                }
                Node::Bindings(bindings) => bindings.split_first().is_none_or(|(first, rest)| {
                    self.push(Node::Expr(&first.value)) && self.push(Node::Bindings(rest))
                }),
            };
            if !pushed {
                return false;
            }
        }
        true
    }
}

impl<N: WordElement> EvmByteArray<N> {
    pub(crate) fn prepare_word_read_with<'a, D: EvmNumericAlgebra<Element = N>>(
        &self,
        offset: &EvmValue<N>,
        domain: AlgebraRef<'a, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Plan<'a, N, D>>, RangeError> {
        self.prepare_word_read_policy(offset, PlanPolicy::Borrowed(domain), budget)
    }
}
impl EvmByteArray<crate::domain::NumericValue> {
    pub(crate) fn prepare_word_read(
        &self,
        offset: &crate::domain::AbstractValue,
        domain: crate::domain::Domain,
        budget: &mut WorkBudget,
    ) -> Result<Option<Plan<'static>>, RangeError> {
        self.prepare_word_read_policy(offset, PlanPolicy::Owned(ValueAlgebra::new(domain)), budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Fork,
        analysis::{ExecutionConfig, OutcomeKind, Status, analyze_world},
        domain::{provenance::Origin, symbolic::SourceId},
        world::{
            Account, Entry, World,
            bytes::{constant, zero},
        },
    };
    use alloy_primitives::Address;

    fn guarded_tree(depth: usize, domain: Domain) -> ByteArray {
        if depth == 0 {
            let mut memory = ByteArray::memory();
            let value = constant(7)
                .with_origin(Origin::Storage)
                .with_expression(ExprId::fresh().unwrap());
            memory.write_word(&zero(), &value, 4096, domain).unwrap();
            return memory;
        }
        guarded_tree(depth - 1, domain)
            .join_guarded(
                &guarded_tree(depth - 1, domain),
                domain,
                ExprId::fresh().unwrap(),
                limits(crate::domain::ValueAlgebra::new(&domain)),
            )
            .unwrap()
    }

    #[test]
    fn three_retained_ites_complete_the_paid_plan_and_native_calldataload() {
        let domain = Domain::default();
        // Three retained state ITEs need seven actual contexts. The former
        // sequence bound rejected the third site as 1 + 6 + 36 > 32.
        let memory = guarded_tree(2, domain);
        let expected = memory.read_word_checked(&zero(), domain).unwrap();
        let mut probe_budget = WorkBudget::new(100_000_000);
        let plan = memory
            .prepare_word_read(&zero(), domain, &mut probe_budget)
            .unwrap()
            .unwrap();
        let preparation = plan.preparation.as_ref().unwrap();
        let context = conditional::Context::prepared(
            &plan.facts,
            &plan.conditional,
            crate::domain::ValueAlgebra::new(&domain),
            preparation.nodes,
        );
        assert_eq!(
            evaluate(
                &plan.root,
                None,
                &expr(&zero()).unwrap(),
                &context,
                crate::domain::ValueAlgebra::new(&domain)
            )
            .unwrap(),
            Some(expected.clone())
        );
        assert!(!context.cache_refused());
        assert_eq!(context.demand_usage().unwrap().0, 7);
        drop(context);
        let trial_work = payer_tests::trial_work(plan, &mut probe_budget);
        // The observation probe and the production read each own a paid plan.
        let mut budget = WorkBudget::new(100_000_000);
        let plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        let before = budget.used();
        let actual = plan.read_word_budgeted(&mut budget).unwrap().unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.numeric(), expected.numeric());
        assert_eq!(actual.provenance(), expected.provenance());
        assert_eq!(actual.expression(), expected.expression());
        assert!(trial_work > 0);
        assert_eq!(
            budget.used(),
            before + trial_work,
            "a completed attempt owes no fallback"
        );

        let calldata = memory.slice(&zero(), &constant(32), 4096, domain).unwrap();
        let owner = Address::repeat_byte(0x11);
        let mut world = World::new(Fork::Osaka, "word-plan:three-retained-ites");
        world
            .insert(owner, Account::from_hex("5f3500", Fork::Osaka).unwrap())
            .unwrap();
        let analysis = analyze_world(
            world,
            Entry::concrete(owner, Address::repeat_byte(0x22), zero(), calldata),
            ExecutionConfig {
                max_work: 100_000_000,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(analysis.config().max_work, 100_000_000);
        assert_eq!(analysis.status(), Status::Converged);
        assert!(analysis.frontiers().is_empty());
        assert_eq!(analysis.states().len(), 1);
        assert_eq!(analysis.states()[0].exit_stack.len(), 1);
        assert_eq!(
            analysis.states()[0].exit_stack[0].singleton(),
            Some(U256::from(7))
        );
        assert!(
            analysis
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return)
        );
    }

    #[test]
    fn more_than_thirty_two_contexts_complete_with_bounded_retention() {
        let domain = Domain::default();
        // Five state-choice levels have 1+2+4+8+16+32 possible contexts.
        let memory = guarded_tree(5, domain);
        let expected = memory.read_word_checked(&zero(), domain).unwrap();
        let mut probe_budget = WorkBudget::new(usize::MAX);
        let plan = memory
            .prepare_word_read(&zero(), domain, &mut probe_budget)
            .unwrap()
            .unwrap();
        let preparation = plan.preparation.as_ref().unwrap();
        let context = conditional::Context::prepared(
            &plan.facts,
            &plan.conditional,
            crate::domain::ValueAlgebra::new(&domain),
            preparation.nodes,
        );
        assert_eq!(
            evaluate(
                &plan.root,
                None,
                &expr(&zero()).unwrap(),
                &context,
                crate::domain::ValueAlgebra::new(&domain)
            )
            .unwrap(),
            Some(expected.clone())
        );
        assert!(!context.cache_refused());
        let (contexts, used) = context.demand_usage().unwrap();
        assert_eq!(contexts, conditional::CONTEXTS);
        assert!(
            used <= preparation.nodes,
            "temporary work stays within the original metadata allowance"
        );
        drop(context);
        let trial_work = payer_tests::trial_work(plan, &mut probe_budget);
        let mut budget = WorkBudget::new(usize::MAX);
        let plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        let before = budget.used();
        let fallback = memory.word_numeric_work(&zero(), domain);
        assert_eq!(plan.fallback_work, fallback);
        let actual = plan.read_word_budgeted(&mut budget).unwrap().unwrap();
        assert!(trial_work > 0);
        assert_eq!(
            budget.used(),
            before + trial_work,
            "full retention alone owes no fallback"
        );
        assert_eq!(actual, expected);
        assert_eq!(actual.numeric(), expected.numeric());
        assert_eq!(actual.provenance(), expected.provenance());
        assert_eq!(actual.identity(), expected.identity());
        assert_eq!(actual.expression(), expected.expression());
        assert_eq!(actual.singleton(), Some(U256::from(7)));
        assert!(!actual.symbolic_limit_reached());
    }

    #[test]
    fn a_small_guarded_word_admits_the_plan_and_scratch_refusal_pays_a_complete_retry() {
        let domain = Domain::default();
        let value = constant(7);
        let source = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            (0usize..2)
                .map(|index| StateBinding {
                    index: U256::from(index),
                    value: ExprId::constant(U256::from(7)),
                    numeric: value.numeric().observation(),
                })
                .collect(),
            limits(crate::domain::ValueAlgebra::new(&domain)).into(),
        )
        .unwrap();
        let mut branches = [ByteArray::memory(), ByteArray::memory()];
        for (index, memory) in branches.iter_mut().enumerate() {
            let value = value.clone().with_origin(Origin::Storage).with_expression(
                ExprId::slot_read(
                    source.clone(),
                    ExprId::constant(U256::from(index)),
                    limits(crate::domain::ValueAlgebra::new(&domain)),
                )
                .unwrap(),
            );
            memory.write_word(&zero(), &value, 4096, domain).unwrap();
        }
        let mut memory = branches[0]
            .join_guarded(
                &branches[1],
                domain,
                ExprId::fresh().unwrap(),
                limits(crate::domain::ValueAlgebra::new(&domain)),
            )
            .unwrap();
        assert!(
            memory.root.nodes() > conditional::CONTEXTS,
            "source definitions are not guard sites"
        );
        let expected = memory.read_word_checked(&zero(), domain).unwrap();
        let mut budget = WorkBudget::new(usize::MAX);
        let plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        assert!(plan.preparation.is_some());
        // The real MLOAD expansion changes allocation metadata after preflight.
        memory
            .expand(&constant(96), &constant(32), 4096, domain)
            .unwrap();
        let mut receipt_budget = WorkBudget::new(usize::MAX);
        let receipt_plan = memory
            .prepare_word_read(&zero(), domain, &mut receipt_budget)
            .unwrap()
            .unwrap();
        let trial_work = payer_tests::trial_work(receipt_plan, &mut receipt_budget);
        let before_read = budget.used();
        assert_eq!(
            plan.read_word_budgeted(&mut budget).unwrap(),
            Some(expected.clone())
        );
        assert!(trial_work > 0);
        assert_eq!(
            budget.used(),
            before_read + trial_work,
            "the admitted demand must finish without a retry"
        );

        let mut plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        plan.preparation.as_mut().unwrap().nodes = 0;
        let mut receipt_plan = memory
            .prepare_word_read(&zero(), domain, &mut receipt_budget)
            .unwrap()
            .unwrap();
        receipt_plan.preparation.as_mut().unwrap().nodes = 0;
        let scratch_work = payer_tests::trial_work(receipt_plan, &mut receipt_budget);
        let before = budget.used();
        let fallback = plan.fallback_work;
        assert_eq!(
            plan.read_word_budgeted(&mut budget).unwrap(),
            Some(expected)
        );
        assert!(scratch_work > 0);
        assert_eq!(
            budget.used(),
            before + scratch_work + fallback,
            "funded fixed setup precedes scratch refusal and the full fallback"
        );

        let mut plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        plan.preparation.as_mut().unwrap().nodes = 0;
        let mut unpaid = WorkBudget::new(0);
        assert_eq!(plan.read_word_budgeted(&mut unpaid).unwrap(), None);
        assert!(unpaid.exhausted());
    }
}
