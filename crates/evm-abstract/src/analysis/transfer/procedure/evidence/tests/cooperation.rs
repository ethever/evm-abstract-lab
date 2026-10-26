//! Positive and retired scope currency for the same native CALL fixture.

use super::*;

#[test]
fn cooperative_exports_follow_selected_body_and_delivery_scope_lifetimes() {
    use crate::analysis::engine::cooperation::Cooperation;
    use crate::analysis::transfer::procedure::{EvmSolver, EvmTable};
    use crate::analysis::{
        EnginePolicies,
        cooperation::{
            Extension, GenerationRoute, Generations, InputRoute, Inputs, Point, PointKind, Subject,
        },
        worklist::{MachineMeter, runtime::Runtime},
    };
    use crate::domain::NumericValue;
    use analysis_kernel::{
        Analyzer, Charges, GenerationEntry, LocalClosure, Read, ScopeKey, Scoped,
    };
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

    // Observe the interpreter's public selectors. The test later retires those
    // real scopes through the kernel protocol, without changing any native row.
    struct Observe(Rc<RefCell<BTreeMap<Subject, GenerationEntry>>>);
    impl<Cx> Analyzer<Cx> for Observe
    where
        Cx: Read<Inputs<NumericValue>, InputRoute>
            + Read<
                Generations,
                GenerationRoute,
                Error = <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error,
            > + Charges<Error = <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error>,
    {
        type Task = Subject;
        type Error = <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error;
        fn evaluate(
            &mut self,
            subject: &Subject,
            cx: &mut Cx,
        ) -> Result<LocalClosure, Self::Error> {
            let input = <Cx as Read<Inputs<NumericValue>, InputRoute>>::query(
                cx,
                Scoped::new(subject.scope, ()),
            )?;
            if let Some(input) = input.value() {
                let entry = <Cx as Read<Generations, GenerationRoute>>::query(
                    cx,
                    Scoped::new(input.universe(), subject.point),
                )?;
                if let Some(entry) = entry.value() {
                    cx.charge(8)?;
                    self.0.borrow_mut().insert(*subject, entry.clone());
                }
            }
            Ok(LocalClosure::Stable)
        }
    }

    let (world, mut entry) = fixture(&[(0x101, "5f5f5f5f5f6101025af15000"), (0x102, "00")]);
    entry.environment.caller = AddressInput::unknown_caller();
    entry.environment.value = TestValue::top();
    entry.environment.calldata = ByteArray::unknown();
    let config = config();
    let algebra = product(NumericLimits::machine().spec());
    let domain = algebra.borrowed();
    let input = initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(config.max_work);
    let mut runtime = Runtime::new(&mut budget).unwrap();
    runtime.register_world(0, 1, &mut budget).unwrap();
    let (_, mut caller) = runtime.next(false, &mut budget).unwrap().unwrap();
    let mut table = EvmTable::default();
    let mut counters = Counters::default();
    let policies = EnginePolicies::default();
    let selectors = Rc::new(RefCell::new(BTreeMap::new()));
    let mut cooperation = Cooperation::new(Extension::new(Observe(Rc::clone(&selectors))));
    let (execution, record) = table.execute_with(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &policies,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
        &mut runtime,
        &mut caller,
        &mut cooperation,
    );
    assert!(record.closed, "{:?}", execution.frontiers);
    assert_indices(&record);
    let cfg = &record.regional_cfg;
    let body = cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer
                .as_ref()
                .is_some_and(|receipt| receipt.current && receipt.complete)
        })
        .unwrap();
    assert_eq!(
        body.transfer.as_ref().unwrap().input_revision,
        body.input_revision
    );
    assert!(
        cfg.nodes
            .iter()
            .all(|node| node.transfer.as_ref().is_none_or(|receipt| receipt.current))
    );
    let call = cfg
        .calls
        .iter()
        .find(|call| !call.deliveries.is_empty())
        .unwrap();
    assert!(
        cfg.calls
            .iter()
            .all(|call| call.deliveries.iter().all(|receipt| receipt.current))
    );
    let contribution = *table
        .exchange_inputs
        .iter()
        .find(|((kind, id, ..), _)| *kind == 1 && *id == call.id)
        .unwrap()
        .1;
    let points = [
        Point {
            namespace: 1,
            kind: PointKind::Body,
            index: body.id,
        },
        Point {
            namespace: 2,
            kind: PointKind::Body,
            index: contribution,
        },
    ];
    for point in points {
        let subject = cooperation.subject(point).unwrap();
        let mut selector = selectors.borrow()[&subject].clone();
        let previous = subject.scope.key();
        let next = runtime
            .kernel()
            .scope(
                ScopeKey {
                    generation: previous.generation + 1,
                    ..previous
                },
                &mut MachineMeter(&mut budget),
            )
            .unwrap();
        runtime
            .kernel()
            .seal_scope(next, &mut MachineMeter(&mut budget))
            .unwrap();
        let mut meter = MachineMeter(&mut budget);
        let mut run = runtime.kernel().resume(&mut caller, &mut meter).unwrap();
        run.replace_generation(&mut selector, next).unwrap();
        caller = run.suspend();
    }
    let mut solver = EvmSolver {
        table: &mut table,
        world: &world,
        entry: &entry,
        config: &config,
        domain,
        policies: &policies,
        budget: &mut budget,
        counters: &mut counters,
        storage_reads: StorageReadPolicy::Abstract,
        runtime: &mut runtime,
        cooperation: &mut cooperation,
        active: None,
        caller: Some(&mut caller),
        namespace: record.version.namespace,
        producer: None,
        deferred: false,
        consumed: None,
    };
    let (rows, retired, _) = solver.semantic_snapshot().unwrap();
    assert!(rows.shares_storage_with(&record.procedures));
    let previous = body.transfer.as_ref().unwrap();
    let current = retired.nodes[body.id].transfer.as_ref().unwrap();
    assert_eq!(current.input_revision, previous.input_revision);
    assert_eq!(current.outcomes, previous.outcomes);
    assert!(
        current.complete && !current.current,
        "complete historical transfer lost its selected basis"
    );
    let deliveries = &retired.calls[call.id].deliveries;
    assert_eq!(deliveries.len(), call.deliveries.len());
    assert!(!deliveries.is_empty());
    for (previous, current) in call.deliveries.iter().zip(deliveries) {
        assert_eq!(current.input_revision, previous.input_revision);
        assert_eq!(current.output_generation, previous.output_generation);
        assert_eq!(current.successors, previous.successors);
        assert!(
            !current.current,
            "same numeric delivery lost its selected contribution scope"
        );
        assert!(
            previous.current,
            "immutable old model retains its original basis"
        );
    }
    assert!(body.transfer.as_ref().unwrap().current);
}
