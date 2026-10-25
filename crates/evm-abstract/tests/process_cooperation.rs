//! An invocation proof outside Process's private-word abstraction feeds an
//! independently scheduled consumer. revm supplies the concrete expectations.
#![allow(dead_code)]

#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;
mod process_cooperation {
    pub(super) mod feedback;
    pub(super) mod phases;
}

use alloy_primitives::{U256, keccak256};
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Capability, Channel, Charges, Cons, Here, JoinPolicy, Kernel,
    KernelError, LocalClosure, Meter, Nil, Pair, PublishError, Read, Reduction, Registered, Scoped,
    There,
};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, ContextInsensitive, EnginePolicies,
        ExecutionConfig, FeedbackAcceleration, KnownProcessSummaries, MachineError,
        MergeResumeStatus, OutcomeKind, ProcessAction, ProcessSource, SemanticPolicies, Status,
        StructuralPartition,
        control::Control,
        cooperation::{
            Extension, ExtraStores, InputRoute, InputView, Inputs, PairedTasks, PerInput,
            PointKind, ProcessFacts, ProcessRefinementRoute, ProcessRefinements, Subject,
        },
    },
    domain::NumericValue,
    world::{EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use std::{cell::Cell, convert::Infallible, rc::Rc};
use support::{A, Assembly, address, entry, insert, new_world};

type Route = There<There<Here>>;
struct ZeroValueSource;
impl Capability for ZeroValueSource {
    type Key = ();
    type Value = bool;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &bool) -> usize {
        1
    }
}
struct Or;
impl JoinPolicy for Or {
    type Value = bool;
    type Error = Infallible;
    fn work(&self, _: &bool, _: &bool) -> usize {
        1
    }
    fn join(&self, old: &bool, next: &bool) -> Result<bool, Infallible> {
        Ok(*old || *next)
    }
    fn equivalent(&self, old: &bool, next: &bool) -> bool {
        old == next
    }
}
struct Stores;
impl ExtraStores for Stores {
    type Stores = Cons<Channel<ZeroValueSource, Ascending<Or>>, Nil>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores.head.len() + 1
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        Ok(Cons::new(kernel.channel(Ascending(Or), meter)?, Nil))
    }
}
#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("{0}")]
    Kernel(#[from] KernelError<MachineError>),
    #[error("{0}")]
    Publish(#[from] PublishError<Infallible, MachineError>),
}

struct Producer {
    enabled: bool,
    bytes: Vec<u8>,
    proved: Rc<Cell<usize>>,
}
impl<Cx> Analyzer<Cx> for Producer
where
    Cx: Read<Inputs<NumericValue>, InputRoute, Error = KernelError<MachineError>>
        + Accepts<ZeroValueSource, Route, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = KernelError<MachineError>>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let input = cx.query(Scoped::new(subject.scope, ()))?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size().saturating_add(self.bytes.len() * 4))?;
        let proved = if let InputView::Process(view) = input.view() {
            cx.charge(view.invocation.value.singleton_work())?;
            let source = match view.control.source {
                ProcessSource::Runtime { hash, .. } | ProcessSource::InputProgram { hash, .. } => {
                    hash
                }
                _ => return publish_false(subject, cx),
            };
            // Independent invariant: the exact byte sequence starts with
            // CALLVALUE ISZERO JUMPI(done), then SSTORE, and done is STOP.
            // A zero invocation therefore never reaches SSTORE. This proof
            // holds for every control location of this source and invocation,
            // including the infeasible write location; it is not a CFG absence
            // claim or an instruction-inventory readonly test.
            self.enabled
                && subject.point.namespace == 1
                && subject.point.kind == PointKind::Body
                && view
                    .program
                    .is_some_and(|program| program.bytes() == self.bytes)
                && source == keccak256(&self.bytes)
                && view.invocation.value.singleton() == Some(U256::ZERO)
        } else {
            false
        };
        if proved {
            self.proved.set(self.proved.get() + 1);
        }
        <Cx as Accepts<ZeroValueSource, Route>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            proved,
        )?;
        Ok(LocalClosure::Stable)
    }
}
fn publish_false<
    Cx: Accepts<ZeroValueSource, Route, Error = PublishError<Infallible, MachineError>>,
>(
    subject: &Subject,
    cx: &mut Cx,
) -> Result<LocalClosure, Failure> {
    cx.accept(Scoped::new(subject.scope, ()), false)?;
    Ok(LocalClosure::Stable)
}
struct Consumer {
    enabled: bool,
    refined: Rc<Cell<usize>>,
}
impl<Cx> Reduction<Cx> for Consumer
where
    Cx: Read<ZeroValueSource, Route, Error = KernelError<MachineError>>
        + Accepts<
            ProcessRefinements,
            ProcessRefinementRoute,
            Error = PublishError<Infallible, MachineError>,
        > + Charges<Error = KernelError<MachineError>>,
{
    type Task = Subject;
    type Error = Failure;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        cx.charge(64)?;
        let proved = cx.query(Scoped::new(subject.scope, ()))?;
        let Some(proved) = proved.value() else {
            return Ok(LocalClosure::Stable);
        };
        let active = self.enabled && *proved;
        let facts = ProcessFacts {
            local_readonly: active,
            excluded_actions: if active {
                [
                    ProcessAction::Call,
                    ProcessAction::StaticCall,
                    ProcessAction::DelegateCall,
                    ProcessAction::CallCode,
                    ProcessAction::Create,
                    ProcessAction::Create2,
                ]
                .into_iter()
                .collect()
            } else {
                Default::default()
            },
        };
        if facts.local_readonly {
            self.refined.set(self.refined.get() + 1);
        }
        cx.accept(Scoped::new(subject.scope, ()), facts)?;
        Ok(LocalClosure::Stable)
    }
}

fn fixture(initial: u16, value: u16) -> (EvmWorld<NumericValue>, EvmEntry<NumericValue>, Vec<u8>) {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::ISZERO)
        .jump("done", true);
    code.store(0, 93);
    code.label("done").op(opcode::STOP);
    let code = code.finish();
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, u64::from(initial))]);
    let mut input = entry(EvmByteArray::empty());
    input.environment.value = support::word(u64::from(value));
    (world, input, code.bytes)
}
fn run(
    initial: u16,
    value: u16,
    producer: bool,
    consumer: bool,
    hybrid: bool,
) -> (
    evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
    usize,
    usize,
) {
    run_source(fixture(initial, value), producer, consumer, hybrid)
}
fn run_source(
    (world, input, bytes): (EvmWorld<NumericValue>, EvmEntry<NumericValue>, Vec<u8>),
    producer: bool,
    consumer: bool,
    hybrid: bool,
) -> (
    evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
    usize,
    usize,
) {
    let proved = Rc::new(Cell::new(0));
    let refined = Rc::new(Cell::new(0));
    // Consumer first: it must wait for the independently authored provider,
    // then wake through the actual kernel dependency and jointly settle.
    let mut extension = Extension::with_tasks(
        Pair::new(
            Registered::new(Consumer {
                enabled: consumer,
                refined: Rc::clone(&refined),
            }),
            Producer {
                enabled: producer,
                bytes,
                proved: Rc::clone(&proved),
            },
        ),
        PairedTasks::new(PerInput, PerInput),
    )
    .with_stores(Stores)
    .with_process_view();
    let values = native::product();
    let policies = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            KnownProcessSummaries,
        ),
    );
    let config = ExecutionConfig {
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let mut engine = AnalysisEngine::new(&values, &policies, &mut extension);
    let analysis = if hybrid {
        engine
            .run_hybrid(
                AnalysisInput::new(world, input).unwrap(),
                config,
                evm_abstract::analysis::HybridConfig {
                    max_detail_work: 0,
                    ..Default::default()
                },
                &Control::default(),
            )
            .unwrap()
            .baseline()
            .clone()
    } else {
        engine
            .run(
                AnalysisInput::new(world, input).unwrap(),
                config,
                &Control::default(),
            )
            .unwrap()
    };
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|record| record.closed && record.source_current)
    );
    for record in analysis.procedure_summaries() {
        assert!(
            record
                .regional_cfg
                .nodes
                .iter()
                .all(|node| node
                    .transfer
                    .as_ref()
                    .is_some_and(|transfer| transfer.current
                        && transfer.complete
                        && !transfer.visited_block))
        );
    }
    (analysis, proved.get(), refined.get())
}
fn exact_slot(
    analysis: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
    initial: u16,
) -> bool {
    analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .all(|outcome| {
            outcome
                .store
                .read(address(A), &support::word(0), native::product().borrowed())
                .singleton()
                == Some(U256::from(initial))
        })
}
#[test]
fn independent_process_pair_improves_effects_and_jointly_closes_with_removal_controls() {
    let (joint, proved, refined) = run(7, 0, true, true, false);
    assert!(proved > 0 && refined > 0);
    assert!(exact_slot(&joint, 7));
    assert!(
        joint
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    for (producer, consumer) in [(false, true), (true, false)] {
        let (control, _, _) = run(7, 0, producer, consumer, false);
        assert!(
            !exact_slot(&control, 7),
            "removing either independent rule must lose the invariant"
        );
    }
    let (world, input, _) = fixture(7, 0);
    let concrete = oracle::execute(&world, &input);
    assert_eq!(concrete.slot(A, 0), U256::from(7));
    assert!(
        !concrete
            .steps
            .iter()
            .any(|step| step.opcode == opcode::SSTORE)
    );
    concrete.covers(&joint);
}
#[test]
fn process_pair_is_supported_in_hybrid_baseline() {
    let (analysis, proved, refined) = run(11, 0, true, true, true);
    assert!(proved > 0 && refined > 0 && exact_slot(&analysis, 11));
}
#[test]
fn nonzero_invocation_is_a_negative_control_for_the_source_proof() {
    let (analysis, proved, refined) = run(7, 1, true, true, false);
    assert_eq!((proved, refined), (0, 0));
    assert!(!exact_slot(&analysis, 7));
    let (world, input, _) = fixture(7, 1);
    let concrete = oracle::execute(&world, &input);
    assert_eq!(concrete.slot(A, 0), U256::from(93));
    assert!(
        concrete
            .steps
            .iter()
            .any(|step| step.opcode == opcode::SSTORE)
    );
    concrete.covers(&analysis);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { failure_persistence: None, ..Default::default() })]
    #[test]
    fn zero_invocation_preserves_arbitrary_original_storage_and_concrete_witness(initial in 0u16..1000) {
        let (joint, proved, refined) = run(initial, 0, true, true, false);
        proptest::prop_assert!(proved > 0 && refined > 0 && exact_slot(&joint, initial));
        let (world, input, _) = fixture(initial, 0);
        let concrete = oracle::execute(&world, &input);
        proptest::prop_assert_eq!(concrete.slot(A, 0), U256::from(initial));
        concrete.covers(&joint);
    }
}

#[test]
fn invocation_proof_excludes_an_actual_process_call_rule_before_subscriber_admission() {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::ISZERO)
        .jump("done", true);
    code.store(0, 93)
        .call(opcode::CALL, 0x202, 9, (0, 0), (0, 0));
    code.label("done").op(opcode::STOP);
    let code = code.finish();
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 7)]);
    let input = entry(EvmByteArray::empty());
    let concrete = oracle::execute(&world, &input);
    assert!(
        !concrete
            .steps
            .iter()
            .any(|step| step.opcode == opcode::CALL)
    );
    let (joint, proved, refined) = run_source((world, input, code.bytes), true, true, false);
    assert!(proved > 0 && refined > 0 && exact_slot(&joint, 7));
    for record in joint.procedure_summaries() {
        assert!(record.regional_cfg.calls.is_empty());
        assert!(record.regional_cfg.nodes.iter().all(|node| {
            node.transfer
                .as_ref()
                .unwrap()
                .process
                .as_ref()
                .unwrap()
                .actions
                .is_empty()
        }));
    }
    concrete.covers(&joint);
}
