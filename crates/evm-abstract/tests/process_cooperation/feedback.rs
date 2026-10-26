//! Invalid representation/scope feedback fails through the typed public API.
use super::super::{native, support};
use analysis_kernel::{
    Accepts, KernelError, LocalClosure, PublishError, Read, Reduction, Registered, Scoped,
};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, ContextInsensitive, EnginePolicies,
        FeedbackAcceleration, KnownProcessSummaries, MachineError, MergeResumeStatus,
        SemanticPolicies, StructuralPartition,
        control::Control,
        cooperation::{
            CooperationError, Extension, InputRoute, InputView, Inputs, ProcessFacts,
            ProcessRefinementRoute, ProcessRefinements, RefinementError, RefinementRoute,
            Refinements, StackFacts, Subject,
        },
    },
    domain::{NumericValue, facts::FactError},
    world::EvmByteArray,
};
use std::convert::Infallible;

#[derive(Debug)]
enum Error {
    Kernel(KernelError<MachineError>),
    Stack(PublishError<FactError, MachineError>),
    Process(PublishError<Infallible, MachineError>),
}
// Keep concrete causes without generating a dynamic Error::source hook.
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kernel(error) => std::fmt::Display::fmt(error, f),
            Self::Stack(error) => std::fmt::Display::fmt(error, f),
            Self::Process(error) => std::fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for Error {}
impl From<KernelError<MachineError>> for Error {
    fn from(error: KernelError<MachineError>) -> Self {
        Self::Kernel(error)
    }
}
impl From<PublishError<FactError, MachineError>> for Error {
    fn from(error: PublishError<FactError, MachineError>) -> Self {
        Self::Stack(error)
    }
}
impl From<PublishError<Infallible, MachineError>> for Error {
    fn from(error: PublishError<Infallible, MachineError>) -> Self {
        Self::Process(error)
    }
}
enum Misuse {
    NativeStack,
    NativeInput,
    LocalWritesWithActions,
}
impl<Cx> Reduction<Cx> for Misuse
where
    Cx: Read<Inputs<NumericValue>, InputRoute, Error = KernelError<MachineError>>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Accepts<
            ProcessRefinements,
            ProcessRefinementRoute,
            Error = PublishError<Infallible, MachineError>,
        >,
{
    type Task = Subject;
    type Error = Error;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Error> {
        let input = cx.query(Scoped::new(subject.scope, ()))?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        match (self, input.view()) {
            (Self::NativeStack, InputView::Process(_)) => {
                <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                    cx,
                    Scoped::new(subject.scope, ()),
                    StackFacts::Unreachable,
                )?;
            }
            (Self::NativeInput, InputView::Native(_))
            | (Self::LocalWritesWithActions, InputView::Process(_)) => {
                <Cx as Accepts<ProcessRefinements, ProcessRefinementRoute>>::accept(
                    cx,
                    Scoped::new(subject.scope, ()),
                    ProcessFacts {
                        local_readonly: true,
                        ..ProcessFacts::none()
                    },
                )?;
            }
            _ => {}
        }
        Ok(LocalClosure::Stable)
    }
}
#[test]
fn representation_mismatches_and_action_effect_scope_are_explicit_errors() {
    for (misuse, expected) in [
        (Misuse::NativeStack, 0),
        (Misuse::NativeInput, 1),
        (Misuse::LocalWritesWithActions, 2),
    ] {
        let mut world = support::new_world();
        let mut code = support::Assembly::default();
        code.call(revm_bytecode::opcode::CALL, 0x202, 9, (0, 0), (0, 0))
            .op(revm_bytecode::opcode::STOP);
        support::insert(&mut world, support::A, &code.finish(), &[]);
        let mut extension = Extension::new(Registered::new(misuse)).with_process_view();
        let policies = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                BoundedJoin,
                FeedbackAcceleration,
                KnownProcessSummaries,
            ),
        );
        let result = AnalysisEngine::new(&native::product(), &policies, &mut extension).run(
            AnalysisInput::new(world, support::entry(EvmByteArray::empty())).unwrap(),
            super::super::ExecutionConfig {
                root_terminal_coverage: false,
                ..Default::default()
            },
            &Control::default(),
        );
        let Err(CooperationError::Refinement(error)) = result else {
            panic!("expected typed refinement error: {result:?}");
        };
        assert!(matches!(
            (expected, error),
            (0, RefinementError::ProcessStack)
                | (1, RefinementError::ProcessScope)
                | (2, RefinementError::ProcessActionEffects)
        ));
    }
}
