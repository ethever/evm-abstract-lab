//! Queue ownership, source-set validity and equal-bound renewal.
use super::*;

#[test]
fn pending_source_availability_wakes_the_real_owner_and_consumes_the_proof_job() {
    use super::super::super::{Binding, Stamp};
    let mut h = Harness::new();
    h.publish().unwrap();
    let point = crate::analysis::cooperation::Point {
        namespace: 1,
        kind: PointKind::Body,
        index: 9,
    };
    let scope = h
        .runtime
        .kernel()
        .scope(
            ScopeKey {
                context: 900,
                generation: 1,
                policy: 0,
            },
            &mut MachineMeter(&mut h.budget),
        )
        .unwrap();
    let subject = Subject { point, scope };
    let seed = h
        .runtime
        .kernel()
        .register(
            scope,
            Task(TaskKind::Seed(subject)),
            &mut MachineMeter(&mut h.budget),
        )
        .unwrap();
    h.runtime
        .kernel()
        .seal_scope(scope, &mut MachineMeter(&mut h.budget))
        .unwrap();
    let relative = h.runtime.relative;
    let entry = h
        .runtime
        .kernel()
        .generation_entry(relative, scope, &mut MachineMeter(&mut h.budget))
        .unwrap();
    let mut meter = MachineMeter(&mut h.budget);
    let mut run = h.runtime.kernel().begin(seed, &mut meter).unwrap();
    let anchor = run.observe_slot(&h.source).unwrap();
    let mut seed = run.suspend();
    // A real registered publisher is suspended before first Input publication.
    // This unit source is not an executed EVM navigation claim.
    let input = Input::new(subject, h.payload.clone()).in_universe(h.runtime.world);
    h.cooperation.bindings.insert(
        point,
        Binding {
            stamp: Stamp {
                anchor,
                phase: 0,
                secondary: None,
                parent: None,
            },
            input: input.clone(),
            entry,
            producer: None,
            consumer: h.writer,
        },
    );
    h.certificate(false);
    let proof_scope = h.cooperation.proof.as_ref().unwrap().scope;
    assert!(
        matches!(h.summary(),analysis_kernel::Availability::Value(summary) if !summary.closed && summary.generation == 7)
    );
    {
        let mut meter = MachineMeter(&mut h.budget);
        let mut run = h.runtime.kernel().resume(&mut seed, &mut meter).unwrap();
        let mut ports = crate::analysis::cooperation::Ports {
            shared: h.cooperation.shared.as_mut().unwrap(),
            local: &mut Nil,
            global: h.cooperation.global.as_mut().unwrap(),
        };
        crate::analysis::cooperation::initialize(&mut run, &mut ports, input).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    assert!(
        !h.runtime
            .view()
            .scope_stale(proof_scope, &mut MachineMeter(&mut h.budget))
            .unwrap(),
        "availability alone must exercise the nonstale queued proof path"
    );
    h.certificate(true); // Runtime must admit SummaryObservation, not SummaryProof.
    assert_eq!(h.cooperation.proof.as_ref().unwrap().scope, proof_scope);
    assert!(
        matches!(h.summary(),analysis_kernel::Availability::Value(summary) if summary.closed && summary.generation == 7)
    );
    assert!(h.runtime.next(false, &mut h.budget).unwrap().is_none());
    assert_eq!(h.runtime.view().closure(), analysis_kernel::Closure::Closed);
}

#[test]
fn new_unseeded_native_source_invalidates_summary_without_changing_entry_or_output() {
    let mut h = Harness::new();
    let readonly = h.publish().unwrap();
    h.certificate(true);
    let original_scope = h.cooperation.proof.as_ref().unwrap().scope;
    let before = match h.summary() {
        analysis_kernel::Availability::Value(summary) => summary,
        other => panic!("{other:?}"),
    };
    h.runtime
        .kernel()
        .retry(h.writer, &mut MachineMeter(&mut h.budget))
        .unwrap();
    let mut writer = h
        .runtime
        .kernel()
        .begin(h.writer, &mut MachineMeter(&mut h.budget))
        .unwrap()
        .suspend();
    let new_body = h
        .runtime
        .register_relative(TaskKind::Body(9), &mut h.budget)
        .unwrap();
    h.cooperation
        .structure_changed(&mut h.runtime, &mut writer, &mut h.budget)
        .unwrap();
    h.runtime.finish(writer, &mut h.budget).unwrap();
    assert!(matches!(
        h.summary(),
        analysis_kernel::Availability::Pending
    ));
    let body = h
        .runtime
        .kernel()
        .begin(new_body, &mut MachineMeter(&mut h.budget))
        .unwrap()
        .suspend();
    h.runtime.finish(body, &mut h.budget).unwrap();
    h.certificate(true);
    let after = match h.summary() {
        analysis_kernel::Availability::Value(summary) => summary,
        other => panic!("{other:?}"),
    };
    assert_eq!(after.generation, before.generation);
    assert_eq!(
        after.revision, before.revision,
        "coverage renewal must not invent observation growth"
    );
    assert_eq!(
        h.cooperation.readonly[&Harness::ID].input.subject(),
        readonly
    );
    let renewed_scope = h.cooperation.proof.as_ref().unwrap().scope;
    assert_ne!(renewed_scope, original_scope);
    h.certificate(true);
    assert_eq!(h.cooperation.proof.as_ref().unwrap().scope, renewed_scope);
    assert!(
        matches!(h.summary(), analysis_kernel::Availability::Value(summary) if summary.revision == after.revision && summary.generation == after.generation)
    );
}

#[test]
fn root_stop_inside_proof_renewal_keeps_the_previous_closed_certificate_unavailable() {
    use crate::analysis::{control::Control, progress::Observer};
    for cancel in [false, true] {
        let mut h = Harness::new();
        h.publish().unwrap();
        h.certificate(true);
        let previous_scope = h.cooperation.proof.as_ref().unwrap().scope;
        h.runtime
            .kernel()
            .retry(h.writer, &mut MachineMeter(&mut h.budget))
            .unwrap();
        let mut writer = h
            .runtime
            .kernel()
            .begin(h.writer, &mut MachineMeter(&mut h.budget))
            .unwrap()
            .suspend();
        let body = h
            .runtime
            .register_relative(TaskKind::Body(9), &mut h.budget)
            .unwrap();
        h.cooperation
            .structure_changed(&mut h.runtime, &mut writer, &mut h.budget)
            .unwrap();
        h.runtime.finish(writer, &mut h.budget).unwrap();
        let body = h
            .runtime
            .kernel()
            .begin(body, &mut MachineMeter(&mut h.budget))
            .unwrap()
            .suspend();
        h.runtime.finish(body, &mut h.budget).unwrap();
        assert!(matches!(
            h.summary(),
            analysis_kernel::Availability::Pending
        ));
        let mut producer = h.producer();
        let control = Control::new(Observer::default());
        let source = &h.source;
        let result = control.scope(|| {
            h.cooperation.publish_summaries(
                vec![(
                    Harness::ID,
                    crate::analysis::cooperation::Summary::new(
                        7,
                        true,
                        std::collections::BTreeMap::new(),
                        8,
                    ),
                )],
                &mut h.runtime,
                &mut producer,
                &mut h.budget,
                |run| {
                    run.observe_slot(source)?;
                    if cancel {
                        control.cancellation().cancel();
                        run.reserve(0)?;
                    } else {
                        run.reserve(1_000_000_001)?;
                    }
                    Ok(())
                },
            )
        });
        assert!(
            matches!(
                result,
                Err(crate::analysis::cooperation::CooperationError::Protocol(
                    KernelError::Stopped(_)
                ))
            ),
            "{result:?}"
        );
        assert_ne!(
            h.cooperation.proof.as_ref().unwrap().scope,
            previous_scope,
            "stop is after selecting the replacement proof, before publication"
        );
        assert!(
            matches!(h.runtime.view().closure(),analysis_kernel::Closure::Open(open) if open.missing > 0),
            "the actual required Summary remains unavailable under the unrefreshed family coverage"
        );
    }
}
