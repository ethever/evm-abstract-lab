//! Two separately paid phases share the kernel's unchanged atomic publication.

use super::{Engine, engine, fixtures};
use crate::{
    analysis::{
        MachineError,
        engine::installation::JoinWork,
        worklist::{
            MachineMeter, kernel_failure,
            runtime::{Task, TaskKind},
        },
    },
    domain::{NumericValue, ValueAlgebra},
    resource::WorkBudget,
};
use analysis_kernel::{Change, LocalClosure, Version};
use std::sync::Arc;

mod words;

const MAXIMUM: usize = 100_000_000;

fn version(engine: &mut Engine<'_>, target: usize) -> Version {
    engine
        .runtime
        .with_run(engine.active.as_mut().unwrap(), engine.budget, |run| {
            run.slot_version(&engine.inputs[target])
                .map_err(kernel_failure)
        })
        .unwrap()
}

/// Bind real readers, then price the native slot's publication in an
/// unpublished fork. Generation/retirement discovery belongs to the kernel,
/// so neither successful nor rejected prefixes are hardcoded here.
fn publication_overhead(engine: &mut Engine<'_>, target: usize) -> usize {
    engine.runtime.requeue_world(target, engine.budget).unwrap();
    let (_, mut reader) = engine.runtime.next(false, engine.budget).unwrap().unwrap();
    let entry = &engine.result.states[target].entry;
    engine
        .runtime
        .with_run(&mut reader, engine.budget, |run| {
            run.read_slot(&engine.inputs[target], entry, entry.work_size())
                .map_err(kernel_failure)?;
            Ok(())
        })
        .unwrap();
    engine.runtime.finish(reader, engine.budget).unwrap();
    let scope = engine.runtime.relative;
    let mut meter = MachineMeter(engine.budget);
    let job = engine
        .runtime
        .kernel()
        .register(scope, Task(TaskKind::Body(usize::MAX)), &mut meter)
        .unwrap();
    let mut run = engine.runtime.kernel().begin(job, &mut meter).unwrap();
    run.read_slot(&engine.inputs[target], entry, entry.work_size())
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(engine.runtime.pending_world().next().is_none());
    assert!(engine.runtime.pending_relative().next().is_none());
    let mut staged = engine.runtime.fork(engine.budget).unwrap();
    let before = engine.budget.used();
    staged
        .with_run(engine.active.as_mut().unwrap(), engine.budget, |run| {
            run.publish_prepared_with(&engine.inputs[target], Change::Changed, 1, || {})
                .map_err(kernel_failure)?;
            Ok(())
        })
        .unwrap();
    engine.budget.used() - before - 1
}

fn rejected_publication_work(
    engine: &mut Engine<'_>,
    target: usize,
    installation: usize,
    allowance: usize,
) -> usize {
    let mut staged = engine.runtime.fork(engine.budget).unwrap();
    let mut quote = WorkBudget::new(allowance);
    let result = staged.with_run(engine.active.as_mut().unwrap(), &mut quote, |run| {
        run.publish_prepared_with(&engine.inputs[target], Change::Changed, installation, || {})
            .map_err(kernel_failure)?;
        Ok(())
    });
    assert_eq!(result, Err(MachineError::Work));
    assert!(quote.exhausted());
    quote.used()
}

#[derive(Clone, Copy, Debug)]
enum Gate {
    Preparation,
    Publication,
    Exact,
    Cancel,
}

#[test]
fn admission_pays_preparation_and_actual_replacement_separately_and_atomically() {
    for gate in [
        Gate::Preparation,
        Gate::Publication,
        Gate::Exact,
        Gate::Cancel,
    ] {
        let cancellation = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&cancellation, || {
            let mut config = fixtures::config();
            config.root_terminal_coverage = false;
            let result = fixtures::run("600100", config);
            let algebra = ValueAlgebra::new(result.config.domain().unwrap());
            let policies = Default::default();
            let mut budget = WorkBudget::new(MAXIMUM);
            let mut counters = super::Counters::default();
            let mut engine = engine(result, 0, &mut budget, &mut counters, &algebra, &policies);
            engine.reset_pending(&[]);
            let kernel_fee = publication_overhead(&mut engine, 0);
            let before_version = version(&mut engine, 0);
            let before_wakes = engine.runtime.view().statistics().wake_attempts;
            let old = engine.result.states[0].entry.clone();
            let history = engine.alternatives[0].work_size();
            let old_selectors = engine.selectors.clone();
            let receipt = engine.result.states[0]
                .execution_evidence()
                .unwrap()
                .is_current();
            let definitions = Arc::clone(
                &engine.result.states[0]
                    .execution_evidence
                    .as_ref()
                    .unwrap()
                    .instruction_values,
            );
            assert_eq!(definitions.len(), 1);
            let mut incoming = old.clone();
            incoming.active_mut().call_value = fixtures::word(1);
            let mut alternatives = engine.alternatives[0].clone();
            let joined = alternatives
                .admit_correlated(
                    &old,
                    &incoming,
                    crate::analysis::Acceleration::Join,
                    &engine.selectors,
                    &mut crate::analysis::PolicyContext::new(
                        engine.domain,
                        &engine.result.entry,
                        engine.budget,
                    ),
                )
                .unwrap()
                .unwrap();
            let costs = JoinWork::quote::<NumericValue>(
                old.work_size(),
                incoming.work_size(),
                joined.work_size(),
                alternatives.work_size(),
                history,
                old_selectors.len(),
            );
            assert!(costs.installation < costs.preparation);
            let needed = costs.preparation + costs.installation + kernel_fee;
            let allowance = match gate {
                Gate::Preparation => costs.preparation - 1,
                Gate::Publication => needed - 1,
                Gate::Exact | Gate::Cancel => needed,
            };
            let rejected_prefix = if matches!(gate, Gate::Publication) {
                rejected_publication_work(
                    &mut engine,
                    0,
                    costs.installation,
                    allowance - costs.preparation,
                )
            } else {
                0
            };
            assert!(
                engine
                    .budget
                    .charge(MAXIMUM - engine.budget.used() - allowance)
            );
            if matches!(gate, Gate::Cancel) {
                cancellation.cancel();
            }
            let spent = engine.budget.used();
            let outcome =
                engine.install_join(0, incoming, Some(joined.clone()), alternatives, history);
            assert!(Arc::ptr_eq(
                &definitions,
                &engine.result.states[0]
                    .execution_evidence
                    .as_ref()
                    .unwrap()
                    .instruction_values,
            ));
            match gate {
                Gate::Exact => {
                    outcome.unwrap();
                    assert_eq!(engine.budget.used() - spent, needed);
                    assert!(!engine.budget.exhausted());
                    assert_eq!(engine.result.states[0].entry, joined);
                    assert!(
                        !engine.result.states[0]
                            .execution_evidence()
                            .unwrap()
                            .is_current()
                    );
                    assert!(engine.runtime.contains_world(&0));
                    assert_eq!(engine.runtime.pending_relative().count(), 1);
                    assert_eq!(
                        engine.runtime.view().statistics().wake_attempts - before_wakes,
                        2
                    );
                }
                Gate::Preparation | Gate::Publication | Gate::Cancel => {
                    let error = outcome.unwrap_err();
                    assert_eq!(
                        error,
                        if matches!(gate, Gate::Cancel) {
                            MachineError::Relations(
                                crate::domain::relational::QueryReason::Cancelled,
                            )
                        } else {
                            MachineError::Work
                        }
                    );
                    assert!(engine.budget.exhausted());
                    assert_eq!(engine.result.states[0].entry, old);
                    assert_eq!(engine.alternatives[0].work_size(), history);
                    assert_eq!(engine.selectors, old_selectors);
                    assert_eq!(
                        engine.result.states[0]
                            .execution_evidence()
                            .unwrap()
                            .is_current(),
                        receipt
                    );
                    assert!(!engine.runtime.contains_world(&0));
                    assert!(engine.runtime.pending_relative().next().is_none());
                    assert_eq!(
                        engine.runtime.view().statistics().wake_attempts,
                        before_wakes
                    );
                    let prefix = if matches!(gate, Gate::Publication) {
                        costs.preparation + rejected_prefix
                    } else {
                        0
                    };
                    assert_eq!(engine.budget.used() - spent, prefix);
                }
            }
            assert_eq!(before_version.get(), 0);
        });
    }
}

#[test]
fn covered_admission_is_free_and_changed_admission_advances_exactly_once() {
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    let result = fixtures::run("00", config);
    let algebra = ValueAlgebra::new(result.config.domain().unwrap());
    let policies = Default::default();
    let mut budget = WorkBudget::new(MAXIMUM);
    let mut counters = super::Counters::default();
    let mut engine = engine(result, 0, &mut budget, &mut counters, &algebra, &policies);
    engine.reset_pending(&[]);
    let before = version(&mut engine, 0);
    let incoming = engine.result.states[0].entry.clone();
    let alternatives = engine.alternatives[0].clone();
    let history = engine.alternatives[0].work_size();
    let spent = engine.budget.used();
    engine
        .install_join(0, incoming.clone(), None, alternatives.clone(), history)
        .unwrap();
    assert_eq!(engine.budget.used(), spent);
    assert_eq!(version(&mut engine, 0), before);
    assert!(!engine.runtime.contains_world(&0));
    let mut incoming = incoming;
    incoming.active_mut().call_value = fixtures::word(1);
    let joined = engine.result.states[0]
        .entry
        .join(&incoming, engine.domain)
        .unwrap();
    engine
        .install_join(0, incoming, Some(joined), alternatives, history)
        .unwrap();
    assert_eq!(version(&mut engine, 0).get(), before.get() + 1);
}

#[test]
fn replacement_quotes_old_history_even_when_the_prepared_history_shrank() {
    let layout = JoinWork::quote::<NumericValue>(0, 0, 0, 0, 0, 0);
    eprintln!(
        "native join transport: preparation={} installation={} (one receipt flag included)",
        layout.preparation, layout.installation,
    );
    let small = JoinWork::quote::<NumericValue>(10, 20, 30, 1, 1, 0);
    let large = JoinWork::quote::<NumericValue>(10, 20, 30, 1, 1_000_001, 0);
    assert_eq!(large.preparation, small.preparation);
    assert_eq!(large.installation - small.installation, 1_000_000);
    let overflow = JoinWork::quote::<NumericValue>(usize::MAX, 1, 1, 1, 1, 1);
    assert_eq!(overflow.preparation, usize::MAX);
    assert_eq!(overflow.installation, usize::MAX);
}
