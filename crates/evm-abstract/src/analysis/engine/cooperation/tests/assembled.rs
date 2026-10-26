//! Live G2 + G5 regression on the selected Process control route.
//! Caller contributions come from actual Native CALL transfer; growth uses the
//! existing paid admission seam, not a mutable patch to an executable machine.
use crate::{
    analysis::{
        AnalysisInput, BoundedJoin, ContextInsensitive, EnginePolicies, EvmMachinePayload,
        EvmMachineState, EvmProcedureSummaryRecord, ExecutionConfig, FeedbackAcceleration,
        KnownProcessSummaries, MachineError, MergeResumeStatus, OutcomeKind, ProcessAction,
        ProcessFrameKey, ProcessLocation, SemanticPolicies, Status, StructuralPartition,
        WholeStatePolicy,
        cooperation::{
            Extension, ExtraStores, InputRoute, InputView, Inputs, PairedTasks, PerInput,
            PointKind, ProcessFacts, ProcessRefinementRoute, ProcessRefinements, SharedRoute,
            SharedStores, Subject,
        },
        engine::{
            Counters, EvmEngine, KeyIndex, PolicyContext, TerminalBoundary,
            cooperation::Cooperation,
        },
        machine::fixtures::{address, config, fixture, word},
        testing::{Account, ByteArray, Entry, World},
        transfer::{self, StorageReadPolicy},
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task, TaskKind},
        },
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};
use alloy_primitives::U256;
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Capability, Channel, Charges, Cons, Either, GenerationEntry,
    Here, JoinPolicy, Kernel, KernelError, LocalClosure, Meter, Nil, Pair, PublishError, Read,
    Reduction, Registered, Scope, ScopeKey, Scoped, There,
};
use std::{cell::RefCell, collections::BTreeSet, convert::Infallible, rc::Rc};

const B: u64 = 0x202;
const CALLEE: &str = "3415600a57605d5f55005b00";
// CALL B with value (CALLDATALOAD(0) & 1); continuation is STOP.
const CALLER: &str = "5f5f5f5f5f356001166102026207a120f100";
type LocalRoute = There<There<Here>>;
type GateRoute = SharedRoute<Here>;
struct Proof;
struct Gate;
impl Capability for Proof {
    type Key = ();
    type Value = bool;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &bool) -> usize {
        1
    }
}
impl Capability for Gate {
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
    fn join(&self, a: &bool, b: &bool) -> Result<bool, Infallible> {
        Ok(*a || *b)
    }
    fn equivalent(&self, a: &bool, b: &bool) -> bool {
        a == b
    }
}
struct LocalFactory;
struct GlobalFactory;
impl ExtraStores for LocalFactory {
    type Stores = Cons<Channel<Proof, Ascending<Or>>, Nil>;
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
impl ExtraStores for GlobalFactory {
    type Stores = Cons<Channel<Gate, Ascending<Or>>, Nil>;
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
#[derive(Debug)]
enum Failure {
    Kernel(KernelError<MachineError>),
    Publish(PublishError<Infallible, MachineError>),
}
// Keep concrete causes without generating a dynamic Error::source hook.
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kernel(error) => std::fmt::Display::fmt(error, f),
            Self::Publish(error) => std::fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for Failure {}
impl From<KernelError<MachineError>> for Failure {
    fn from(error: KernelError<MachineError>) -> Self {
        Self::Kernel(error)
    }
}
impl From<PublishError<Infallible, MachineError>> for Failure {
    fn from(error: PublishError<Infallible, MachineError>) -> Self {
        Self::Publish(error)
    }
}
#[derive(Clone, Debug)]
struct Observation {
    subject: Subject,
    control: ProcessFrameKey,
    values: BTreeSet<U256>,
    proved: bool,
}
type Trace = Rc<RefCell<Vec<Observation>>>;
struct Producer {
    enabled: bool,
    trace: Trace,
}
impl<Cx> Analyzer<Cx> for Producer
where
    Cx: Read<Inputs<NumericValue>, InputRoute, Error = KernelError<MachineError>>
        + Read<Gate, GateRoute, Error = KernelError<MachineError>>
        + Accepts<Proof, LocalRoute, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = KernelError<MachineError>>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let input = <Cx as Read<Inputs<NumericValue>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size().saturating_add(512))?;
        let mut proved = false;
        if subject.point.namespace == 1
            && subject.point.kind == PointKind::Body
            && let InputView::Process(view) = input.view()
            && view.program.is_some_and(|p| {
                p.bytes()
                    == [
                        0x34, 0x15, 0x60, 0x0a, 0x57, 0x60, 0x5d, 0x5f, 0x55, 0x00, 0x5b, 0x00,
                    ]
            })
        {
            let gate = <Cx as Read<Gate, GateRoute>>::query(cx, Scoped::new(input.universe(), ()))?;
            let Some(gate) = gate.value() else {
                return Ok(LocalClosure::Stable);
            };
            cx.charge(view.invocation.value.singleton_work())?;
            // Exact source: zero CALLVALUE branches directly to STOP. This
            // invariant covers every location, including the infeasible write
            // block, independently of Step::at's erased-word control analysis.
            proved = self.enabled && *gate && view.invocation.value.singleton() == Some(U256::ZERO);
            self.trace.borrow_mut().push(Observation {
                subject: *subject,
                control: view.control.clone(),
                values: view.invocation.value.constants().cloned().unwrap(),
                proved,
            });
        }
        <Cx as Accepts<Proof, LocalRoute>>::accept(cx, Scoped::new(subject.scope, ()), proved)?;
        Ok(LocalClosure::Stable)
    }
}
struct Consumer {
    enabled: bool,
}
impl<Cx> Reduction<Cx> for Consumer
where
    Cx: Read<Proof, LocalRoute, Error = KernelError<MachineError>>
        + Accepts<
            ProcessRefinements,
            ProcessRefinementRoute,
            Error = PublishError<Infallible, MachineError>,
        > + Charges<Error = KernelError<MachineError>>,
{
    type Task = Subject;
    type Error = Failure;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        cx.charge(128)?;
        let proof = cx.query(Scoped::new(subject.scope, ()))?;
        let Some(proof) = proof.value() else {
            return Ok(LocalClosure::Stable);
        };
        let mut facts = ProcessFacts::none();
        if self.enabled && *proof {
            facts.local_readonly = true;
            facts.excluded_actions.extend([
                ProcessAction::Call,
                ProcessAction::StaticCall,
                ProcessAction::DelegateCall,
                ProcessAction::CallCode,
                ProcessAction::Create,
                ProcessAction::Create2,
            ]);
        }
        cx.accept(Scoped::new(subject.scope, ()), facts)?;
        Ok(LocalClosure::Stable)
    }
}
type Assembly = Extension<
    Pair<Registered<Consumer>, Producer>,
    PairedTasks<PerInput, PerInput>,
    LocalFactory,
    GlobalFactory,
>;
type State =
    SemanticPolicies<StructuralPartition, BoundedJoin, FeedbackAcceleration, KnownProcessSummaries>;
type Policies = EnginePolicies<ContextInsensitive, MergeResumeStatus, State>;
type Engine<'a> = EvmEngine<
    'a,
    ProductAlgebra,
    ContextInsensitive,
    MergeResumeStatus,
    State,
    Assembly,
    Either<Failure, Failure>,
>;

fn contribution(
    world: &World,
    entry: &Entry,
    value: u64,
    values: &ValueAlgebra<ProductAlgebra>,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
) -> EvmMachinePayload<NumericValue> {
    let mut concrete = entry.clone();
    concrete.environment.calldata = ByteArray::exact(&U256::from(value).to_be_bytes::<32>());
    let initial = transfer::initial(world, &concrete, values.borrowed()).unwrap();
    let execution = transfer::execute(
        world,
        &concrete,
        initial,
        config,
        values.borrowed(),
        &ContextInsensitive,
        budget,
        StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty());
    let child = execution
        .successors
        .into_iter()
        .find(|s| s.kind == crate::analysis::MachineEdgeKind::Call)
        .unwrap()
        .payload;
    assert_eq!(child.active().key.address, address(B));
    assert_eq!(
        child.active().call_value.singleton(),
        Some(U256::from(value))
    );
    child
}

/// A live engine starting at a real CALL contribution, like existing private
/// World/procedure tests. The original caller input remains unknown calldata;
/// the two concrete prefixes witness distinct legitimate contributors. No
/// oracle is produced by these transfers: the expected write is literal93.
fn engine<'a>(
    values: &'a ValueAlgebra<ProductAlgebra>,
    policies: &'a Policies,
    budget: &'a mut WorkBudget,
    counters: &'a mut Counters,
    producer: bool,
    consumer: bool,
) -> (
    Engine<'a>,
    EvmMachinePayload<NumericValue>,
    Trace,
    GenerationEntry,
    Scope,
) {
    let (_, mut entry) = fixture(&[]);
    entry.environment.calldata = ByteArray::unknown();
    let mut world = World::new(crate::Fork::Osaka, "assembled Process contributors");
    for (owner, code) in [(0x101, CALLER), (B, CALLEE)] {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = word(1_000_000);
        account.storage.insert(U256::ZERO, word(7));
        world.insert(address(owner), account).unwrap();
    }
    let mut config = config();
    config.use_summaries = true;
    config.root_terminal_coverage = false;
    let first = contribution(&world, &entry, 0, values, &config, budget);
    let second = contribution(&world, &entry, 1, values, &config, budget);
    assert_eq!(first.key(), second.key());
    let input = AnalysisInput::new(world, entry).unwrap();
    let result = crate::analysis::EvmWorldAnalysis {
        snapshot: input.snapshot().clone(),
        revision: input.revision(),
        world: input.world,
        entry: input.entry,
        config,
        schema_version: crate::analysis::SCHEMA_VERSION,
        domain_spec: values.borrowed().spec(),
        states: vec![EvmMachineState {
            id: 0,
            key: first.key(),
            entry: first,
            exit: None,
            exit_stack: Vec::new(),
            executed_pcs: Vec::new(),
            execution_evidence: None,
            resume_status: None,
        }],
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        root_terminal_coverage: None,
        status: Status::Incomplete,
        transfers: 0,
        work: 0,
        rpc_acquisition: None,
        summary_stats: Default::default(),
        summaries: Vec::new(),
        procedure_summaries: Default::default(),
    };
    let trace = Rc::new(RefCell::new(Vec::new()));
    let extension = Extension::with_tasks(
        Pair::new(
            Registered::new(Consumer { enabled: consumer }),
            Producer {
                enabled: producer,
                trace: Rc::clone(&trace),
            },
        ),
        PairedTasks::new(PerInput, PerInput),
    )
    .with_stores(LocalFactory)
    .with_shared_stores(GlobalFactory)
    .with_process_view();
    let mut runtime = Runtime::new(budget).unwrap();
    let mut cooperation = Cooperation::new(extension);
    let global = cooperation
        .assembly
        .create_shared(runtime.kernel(), &mut MachineMeter(budget))
        .unwrap();
    cooperation.global = Some(global);
    let world_scope = runtime.world;
    let foreign = runtime
        .kernel()
        .scope(
            ScopeKey {
                context: 1_000_000,
                generation: 0,
                policy: 0,
            },
            &mut MachineMeter(budget),
        )
        .unwrap();
    let job = runtime
        .kernel()
        .register(
            foreign,
            Task(TaskKind::SummaryProof),
            &mut MachineMeter(budget),
        )
        .unwrap();
    runtime
        .kernel()
        .seal_scope(foreign, &mut MachineMeter(budget))
        .unwrap();
    let foreign_entry = runtime
        .kernel()
        .generation_entry(world_scope, foreign, &mut MachineMeter(budget))
        .unwrap();
    let mut meter = MachineMeter(budget);
    let mut run = runtime.kernel().begin(job, &mut meter).unwrap();
    run.publish(
        &mut cooperation.global.as_mut().unwrap().head,
        Scoped::new(world_scope, ()),
        true,
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let input_slot = runtime
        .kernel()
        .slot(world_scope, &mut MachineMeter(budget))
        .unwrap();
    runtime.register_world(0, 2, budget).unwrap();
    let mut ids = KeyIndex::new();
    ids.insert_with((result.states[0].key.clone(), ()), budget, |_| Ok(0))
        .unwrap();
    let metadata = policies
        .state
        .initialize(
            &result.states[0].entry,
            &mut PolicyContext::new(values.borrowed(), &result.entry, budget),
        )
        .unwrap();
    counters.states = 1;
    let engine = EvmEngine {
        result,
        domain: values.borrowed(),
        policies,
        budget,
        counters,
        storage_reads: StorageReadPolicy::Abstract,
        alternatives: vec![metadata],
        selectors: BTreeSet::new(),
        ids,
        partitions: vec![()],
        updates: vec![0],
        feedback: vec![false],
        runtime,
        cooperation: Some(cooperation),
        inputs: vec![input_slot],
        active: None,
        active_node: None,
        test_driver: None,
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new()],
        source_records: vec![None],
        cache: None,
        procedures: Default::default(),
        reuse: None,
        terminal_boundary: TerminalBoundary::default(),
    };
    (engine, second, trace, foreign_entry, foreign)
}
fn policies() -> Policies {
    EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(SemanticPolicies::new(
        StructuralPartition,
        BoundedJoin,
        FeedbackAcceleration,
        KnownProcessSummaries,
    ))
}
fn record<'e>(engine: &'e Engine<'_>) -> &'e EvmProcedureSummaryRecord<NumericValue> {
    engine
        .result
        .procedure_summaries()
        .iter()
        .rev()
        .find(|r| r.source_state == 0)
        .unwrap()
}
fn slot(
    record: &EvmProcedureSummaryRecord<NumericValue>,
) -> &crate::domain::EvmValue<NumericValue> {
    record.procedures[record.root_procedure.unwrap()]
        .outputs
        .iter()
        .find(|o| o.kind == OutcomeKind::Return)
        .unwrap()
        .store
        .storage_bound(address(B), U256::ZERO)
}
fn source_version(engine: &mut Engine<'_>) -> analysis_kernel::Version {
    let (_, mut driver) = engine.runtime.test_driver(engine.budget).unwrap();
    let version = engine
        .runtime
        .with_run(&mut driver, engine.budget, |run| {
            Ok(run.slot_version(&engine.inputs[0]).unwrap())
        })
        .unwrap();
    engine.runtime.finish(driver, engine.budget).unwrap();
    version
}

#[test]
fn assembled_pair_gain_disappears_when_either_rule_is_removed() {
    for (producer, consumer) in [(true, true), (false, true), (true, false)] {
        let values = ValueAlgebra::new(ProductAlgebra::default());
        let policies = policies();
        let mut budget = WorkBudget::new(config().max_work);
        let mut counters = Counters::default();
        let (mut engine, _, _, _, _) = engine(
            &values,
            &policies,
            &mut budget,
            &mut counters,
            producer,
            consumer,
        );
        engine.run();
        let model = record(&engine);
        assert!(
            model.closed && model.source_current,
            "{:?}",
            engine.result.frontiers()
        );
        assert_eq!(
            slot(model).singleton() == Some(U256::from(7)),
            producer && consumer
        );
    }
}

#[test]
fn assembled_invocation_growth_retires_the_old_proof_and_covers_the_write() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let policies = policies();
    let mut budget = WorkBudget::new(config().max_work);
    let mut counters = Counters::default();
    let (mut engine, second, trace, _, _) =
        engine(&values, &policies, &mut budget, &mut counters, true, true);
    engine.run();
    let old = record(&engine).clone();
    assert!(old.closed && old.source_current);
    assert_eq!(slot(&old).singleton(), Some(U256::from(7)));
    let witness = trace
        .borrow()
        .iter()
        .find(|o| o.control.location == ProcessLocation::Block(0) && o.proved)
        .unwrap()
        .clone();
    let native_version = source_version(&mut engine);
    let (_, driver) = engine.runtime.test_driver(engine.budget).unwrap();
    engine.active = Some(driver);
    let work = <State as WholeStatePolicy<ProductAlgebra>>::metadata_work(
        &engine.policies.state,
        &engine.alternatives[0],
    );
    engine.admit_existing(0, &second, work).unwrap();
    engine
        .runtime
        .finish(engine.active.take().unwrap(), engine.budget)
        .unwrap();
    assert_eq!(engine.source_records[0], None);
    assert!(source_version(&mut engine) > native_version);
    engine.certify_procedure_sources();
    assert!(!record(&engine).source_current);
    engine.run();
    let current = record(&engine).clone();
    assert!(
        current.closed && current.source_current,
        "{:?}",
        engine.result.frontiers()
    );
    let root = &current.procedures[current.root_procedure.unwrap()];
    assert_eq!(
        root.process_input.as_ref().unwrap().value.constants(),
        Some(&BTreeSet::from([U256::ZERO, U256::ONE]))
    );
    assert!(slot(&current).contains(U256::from(93)));
    assert!(engine.result.outcomes().iter().any(|o| {
        o.kind == OutcomeKind::Return
            && o.store
                .read(address(B), &word(0), values.borrowed())
                .contains(U256::from(93))
    }));
    let next = trace
        .borrow()
        .iter()
        .find(|o| {
            o.subject.point == witness.subject.point
                && o.control == witness.control
                && o.subject.scope != witness.subject.scope
                && o.values == BTreeSet::from([U256::ZERO, U256::ONE])
                && !o.proved
        })
        .cloned();
    assert!(
        next.is_some(),
        "same owner/source/control must receive the expanded invocation in a fresh scope"
    );
    assert!(
        engine
            .runtime
            .view()
            .scope_stale(witness.subject.scope, &mut MachineMeter(engine.budget))
            .unwrap()
    );
    assert!(
        current
            .regional_cfg
            .nodes
            .iter()
            .any(|n| n.frame.process.as_deref() == Some(&witness.control)
                && n.transfer.as_ref().is_some_and(|t| t.current && t.complete))
    );
    assert!(old.closed && old.source_current);
    assert_eq!(slot(&old).singleton(), Some(U256::from(7)));
    assert_eq!(
        old.procedures[old.root_procedure.unwrap()]
            .process_input
            .as_ref()
            .unwrap()
            .value
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn assembled_foreign_retirement_invalidates_currency_without_changing_numeric_rows() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let policies = policies();
    let mut budget = WorkBudget::new(config().max_work);
    let mut counters = Counters::default();
    let (mut engine, _, trace, mut foreign_entry, foreign) =
        engine(&values, &policies, &mut budget, &mut counters, true, true);
    engine.run();
    // Refresh this source after the caller continuation has also settled, so
    // the retained directory covers the same complete table on both sides.
    engine.runtime.requeue_world(0, engine.budget).unwrap();
    engine.run();
    let old = record(&engine).clone();
    assert!(old.closed && old.source_current);
    assert_eq!(slot(&old).singleton(), Some(U256::from(7)));
    let witness = trace
        .borrow()
        .iter()
        .find(|o| o.control.location == ProcessLocation::Block(0) && o.proved)
        .unwrap()
        .clone();
    let native_version = source_version(&mut engine);
    let updates = engine.updates.clone();
    let outcomes = engine.result.outcomes.len();
    let successors = engine.result.states[0]
        .execution_evidence()
        .unwrap()
        .successors()
        .to_vec();
    assert!(
        !successors.is_empty(),
        "real CALL result reaches its caller"
    );
    let next = engine
        .runtime
        .kernel()
        .scope(
            ScopeKey {
                context: foreign.key().context,
                generation: 1,
                policy: 0,
            },
            &mut MachineMeter(engine.budget),
        )
        .unwrap();
    let seed = engine
        .runtime
        .kernel()
        .register(
            next,
            Task(TaskKind::SummaryProof),
            &mut MachineMeter(engine.budget),
        )
        .unwrap();
    engine
        .runtime
        .kernel()
        .seal_scope(next, &mut MachineMeter(engine.budget))
        .unwrap();
    let (_, mut driver) = engine.runtime.test_driver(engine.budget).unwrap();
    engine
        .runtime
        .with_run(&mut driver, engine.budget, |run| {
            run.replace_generation(&mut foreign_entry, next).unwrap();
            Ok(())
        })
        .unwrap();
    engine.runtime.finish(driver, engine.budget).unwrap();
    assert_eq!(source_version(&mut engine), native_version);
    assert!(
        engine
            .runtime
            .view()
            .scope_stale(witness.subject.scope, &mut MachineMeter(engine.budget))
            .unwrap()
    );
    engine.certify_procedure_sources();
    assert!(!record(&engine).source_current);
    let observations = engine.result.procedure_summaries().len();
    // No manual World retry: the native owner clock must wake this consumer
    // even though the procedure's numerical output never changed.
    engine.run();
    assert!(engine.result.procedure_summaries().len() > observations);
    assert_eq!(source_version(&mut engine), native_version);
    let pending = record(&engine).clone();
    assert!(!pending.closed && !pending.source_current);
    assert!(
        pending.procedures.shares_storage_with(&old.procedures),
        "numeric directory unchanged while foreign premise is unavailable"
    );
    assert!(
        pending
            .regional_cfg
            .nodes
            .iter()
            .any(|n| n.frame.process.as_deref() == Some(&witness.control)
                && n.transfer.as_ref().is_some_and(|t| !t.current))
    );
    assert_eq!(
        engine.updates, updates,
        "reproof does not readmit caller inputs"
    );
    assert_eq!(
        engine.result.outcomes.len(),
        outcomes,
        "reproof does not redeliver terminal effects"
    );
    assert_eq!(
        engine.result.states[0]
            .execution_evidence()
            .unwrap()
            .successors(),
        successors
    );
    assert!(old.closed && old.source_current);
    assert_eq!(slot(&old).singleton(), Some(U256::from(7)));
    assert!(
        old.regional_cfg
            .nodes
            .iter()
            .all(|n| n.transfer.as_ref().is_some_and(|t| t.current))
    );
    let world = engine.runtime.world;
    let mut meter = MachineMeter(engine.budget);
    let mut run = engine.runtime.kernel().begin(seed, &mut meter).unwrap();
    run.publish(
        &mut engine
            .cooperation
            .as_mut()
            .unwrap()
            .global
            .as_mut()
            .unwrap()
            .head,
        Scoped::new(world, ()),
        true,
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    engine.run();
    let renewed = record(&engine);
    assert!(
        renewed.closed && renewed.source_current,
        "{:?}",
        engine.result.frontiers()
    );
    assert_eq!(slot(renewed).singleton(), Some(U256::from(7)));
    assert_eq!(
        engine.updates, updates,
        "same-value reproof preserves caller inputs"
    );
    assert_eq!(
        engine.result.outcomes.len(),
        outcomes,
        "same-value reproof preserves terminal delivery"
    );
    assert_eq!(
        engine.result.states[0]
            .execution_evidence()
            .unwrap()
            .successors(),
        successors
    );
    assert!(renewed.version.generation > old.version.generation);
    assert!(
        trace
            .borrow()
            .iter()
            .any(|o| o.subject.point == witness.subject.point
                && o.subject.scope != witness.subject.scope
                && o.proved)
    );
}
