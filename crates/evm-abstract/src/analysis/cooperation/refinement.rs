use super::{
    Input, InputAlreadyInitialized, InputRoute, Inputs, Ports, RefinementRoute, Refinements,
    Shared, StackFacts, Subject,
};
use crate::{
    analysis::{EvmMachinePayload, MachineError},
    domain::{
        AlgebraRef, EvmNumericAlgebra, ReductionStatus, WordElement,
        facts::{FactError, Symbol},
    },
    resource::{WorkBudget, clone_payment},
};
use analysis_kernel::{
    Accepts, Analyzer, Answer, Capability, Context, KernelError, LocalClosure, Meter, PublishError,
    Reduction, Registered, Run, Scoped,
};

/// Failed preparation is not a proof that the EVM input is unreachable.
#[derive(Debug, thiserror::Error)]
pub enum InitializationError<S> {
    /// The native retained input could not be materialized under the root meter.
    #[error("cooperation input materialization failed: {0}")]
    Materialization(S),
    /// The root ledger or scope protocol refused required seed construction.
    #[error("cooperation initialization failed: {0}")]
    Kernel(KernelError<S>),
    /// A seed was evaluated outside its declared immutable input universe.
    #[error("cooperation input seed belongs to a different scope")]
    WrongScope,
    /// Input publication was refused; an existing input cannot be overwritten.
    #[error("cooperation input publication failed: {0}")]
    Input(PublishError<InputAlreadyInitialized, S>),
    /// Registered initial guarantees could not be installed completely.
    #[error("cooperation feedback initialization failed: {0}")]
    Feedback(PublishError<FactError, S>),
}

/// Explicitly registered initialization accepts the same fixed-input semantic
/// contract as later reductions. Normal analyzers have no first-write bypass.
struct InitializeInput<'a, N: WordElement> {
    input: &'a Input<N>,
}

impl<Cx, N: WordElement> Reduction<Cx> for InitializeInput<'_, N>
where
    Cx: Accepts<Inputs<N>, InputRoute>,
{
    type Task = Subject;
    type Error = <Cx as Accepts<Inputs<N>, InputRoute>>::Error;

    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.accept(Scoped::new(subject.scope, ()), Input::clone(self.input))?;
        Ok(LocalClosure::Stable)
    }
}

struct InitializeFeedback;

impl<Cx> Reduction<Cx> for InitializeFeedback
where
    Cx: Accepts<Refinements, RefinementRoute>,
{
    type Task = Subject;
    type Error = <Cx as Accepts<Refinements, RefinementRoute>>::Error;

    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.accept(Scoped::new(subject.scope, ()), StackFacts::none())?;
        Ok(LocalClosure::Stable)
    }
}

/// Called on a bootstrap job in the NEW input scope, before publishing its
/// stable generation selector. Failure leaves that bootstrap obligation open.
pub(crate) fn initialize<N: WordElement, J, M: Meter, Tail, Global>(
    run: &mut Run<'_, J, M>,
    ports: &mut Ports<'_, N, Tail, Global>,
    input: Input<N>,
) -> Result<(), InitializationError<M::Stop>> {
    initialize_input(run, ports, &input)?;
    initialize_feedback(run, ports, input.subject())
}

/// Resume a bootstrap interrupted after Input publication, without overwriting
/// that immutable Input or resetting any already published local guarantees.
pub(crate) fn initialize_feedback<N: WordElement, J, M: Meter, Tail, Global>(
    run: &mut Run<'_, J, M>,
    ports: &mut Ports<'_, N, Tail, Global>,
    subject: Subject,
) -> Result<(), InitializationError<M::Stop>> {
    let mut cx = Context::new(run, ports);
    Registered::new(InitializeFeedback)
        .evaluate(&subject, &mut cx)
        .map_err(InitializationError::Feedback)?;
    Ok(())
}

/// Seed only the immutable observation; this scope is not executable and has
/// no client task inventory, generation-local stores or Refinements cell.
pub(crate) fn initialize_readonly<N: WordElement, J, M: Meter>(
    run: &mut Run<'_, J, M>,
    shared: &mut Shared<N>,
    input: &Input<N>,
) -> Result<(), InitializationError<M::Stop>> {
    let mut ports = Ports {
        shared,
        local: &mut analysis_kernel::Nil,
        global: &mut analysis_kernel::Nil,
    };
    initialize_input(run, &mut ports, input)
}

fn initialize_input<N: WordElement, J, M: Meter, Tail, Global>(
    run: &mut Run<'_, J, M>,
    ports: &mut Ports<'_, N, Tail, Global>,
    input: &Input<N>,
) -> Result<(), InitializationError<M::Stop>> {
    let subject = input.subject();
    if run.scope() != subject.scope {
        return Err(InitializationError::WrongScope);
    }
    run.reserve(Inputs::<N>::value_work(input))
        .map_err(InitializationError::Kernel)?;
    let mut cx = Context::new(run, ports);
    Registered::new(InitializeInput { input })
        .evaluate(&subject, &mut cx)
        .map_err(InitializationError::Input)?;
    Ok(())
}

/// Interpreter consumers read feedback through this same tracked publication
/// channel. Retired subjects fail through the kernel's scope validation; pending
/// answers retain a mandatory read obligation and never imply unreachable.
pub(crate) fn read_refinements<N: WordElement, J, M: Meter>(
    run: &mut Run<'_, J, M>,
    shared: &mut Shared<N>,
    subject: Subject,
) -> Result<Answer<StackFacts>, KernelError<M::Stop>> {
    run.query(&mut shared.refinements, Scoped::new(subject.scope, ()))
}

/// A detached executable view, with contradiction separate from interruption.
#[derive(Debug)]
pub enum LocalInput<N: WordElement> {
    /// Required provider work is pending; the same Kernel retries the native
    /// owner after its client scope settles, including equivalent Top answers.
    Deferred,
    /// A valid numerical contradiction in this fixed input universe.
    Unreachable,
    /// The original carrier and metadata survive; precision limits do not remove
    /// this path. Global anchors and previous generations are never mutated.
    Reachable {
        /// Owned detached input; unboxing transfers it to bytecode transfer
        /// without cloning the actual numeric carrier or its metadata.
        payload: Box<EvmMachinePayload<N>>,
        /// Optional bounded numerical closure results, for native diagnostics.
        limited: Vec<ReductionStatus>,
    },
}

/// Typed local refinement refusal, distinct from a semantic contradiction.
#[derive(Debug, thiserror::Error)]
pub enum RefinementError {
    /// Original root work ledger or cancellation refused required work.
    #[error("{0}")]
    Stopped(MachineError),
    /// Feedback references no operand in this immutable input generation.
    #[error("cooperation stack slot {slot} is outside the active stack height {height}")]
    StackSlot {
        /// Bottom-based stack index used by the registered component.
        slot: usize,
        /// Actual fixed input height.
        height: usize,
    },
    /// Invalid/non-numerical facts cannot be turned into numerical Bottom.
    #[error("{0}")]
    Facts(FactError),
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), RefinementError> {
    clone_payment::checkpoint(budget).map_err(|error| RefinementError::Stopped(error.into()))?;
    if work == usize::MAX || !budget.charge(work) {
        return Err(RefinementError::Stopped(MachineError::work_failure()));
    }
    Ok(())
}

/// Consume the already paid detached input from the interpreter's tracked read.
/// Only its active scalar operands change, using the run's actual D. A failure
/// discards this view while the authoritative ascending input remains untouched.
pub(crate) fn apply_refinements<D: EvmNumericAlgebra>(
    mut payload: EvmMachinePayload<D::Element>,
    facts: &StackFacts,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<LocalInput<D::Element>, RefinementError> {
    let StackFacts::Reachable(facts) = facts else {
        return Ok(LocalInput::Unreachable);
    };
    let height = payload.active().stack.len();
    let mut limited = Vec::new();
    for (slot, observation) in facts {
        let Some(value) = payload.active().stack.get(*slot) else {
            return Err(RefinementError::StackSlot {
                slot: *slot,
                height,
            });
        };
        reserve(
            budget,
            domain
                .refinement_work(value, observation.work_size())
                .saturating_add(value.clone_work()),
        )?;
        let numeric = domain.constrain_numeric(value.numeric(), observation);
        // Opaque hooks may cancel either owner, even when returning an error or
        // a refused reduction. Preserve the paid prefix and stop before copying.
        clone_payment::checkpoint(budget)
            .map_err(|error| RefinementError::Stopped(error.into()))?;
        let numeric = match numeric {
            Ok(result) => result,
            Err(FactError::Contradiction { .. }) => return Ok(LocalInput::Unreachable),
            Err(error) => return Err(RefinementError::Facts(error)),
        };
        match numeric.status {
            ReductionStatus::Empty => {
                // Empty retains a conservative prior result, not the explicit
                // numerical contradiction proved by constrain().
                reserve(budget, 1)?;
                limited.push(ReductionStatus::Empty);
                continue;
            }
            ReductionStatus::OriginConflict => {
                return Err(RefinementError::Facts(FactError::OriginContradiction {
                    subject: Symbol::THIS,
                }));
            }
            ReductionStatus::Stable => {}
            status => {
                reserve(budget, 1)?;
                limited.push(status);
            }
        }
        let next = payload.active().stack[*slot]
            .clone()
            .with_numeric(numeric.value);
        payload.active_mut().stack[*slot] = next;
    }
    // The detached state and scalar copies were paid by their callers above.
    // This new allocation only moves the shallow payload layout into the box;
    // do not charge another deep traversal/clone of its frames or numeric values.
    let allocation_work = 1usize.saturating_add(
        std::mem::size_of::<EvmMachinePayload<D::Element>>().div_ceil(std::mem::size_of::<usize>()),
    );
    reserve(budget, allocation_work)?;
    Ok(LocalInput::Reachable {
        payload: Box::new(payload),
        limited,
    })
}
