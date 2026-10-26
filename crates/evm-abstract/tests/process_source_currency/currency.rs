use crate::{
    A, Assembly, address, assert_current, config, entry, evidence, fixture, insert, native,
    new_world, policies, root_epoch, run,
};
use alloy_primitives::keccak256;
use analysis_kernel::{Analyzer, LocalClosure};
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, ProcessSource,
        control::Control,
        cooperation::{CooperationError, Extension, Subject},
    },
    world::{EvmByteArray, EvmWorld},
};
use revm_bytecode::opcode;
use std::convert::Infallible;

#[test]
fn independent_captured_byte_or_fork_changes_have_disjoint_current_source_keys() {
    let mut seen = std::collections::BTreeSet::new();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for bytes in [&[opcode::STOP][..], &[opcode::PUSH0, opcode::STOP][..]] {
            let mut world = EvmWorld::new(fork, "process-source-currency");
            insert(&mut world, A, &fixture(bytes), &[]);
            let analysis = run(world);
            assert_current(&analysis);
            assert!(seen.insert(root_epoch(&analysis)));
            for record in analysis.procedure_summaries() {
                let root = &record.procedures[record.root_procedure.unwrap()];
                assert_eq!(
                    root.frame.process.as_ref().unwrap().source,
                    ProcessSource::Runtime {
                        source: address(A),
                        hash: keccak256(bytes),
                        fork
                    }
                );
            }
        }
    }
}

#[test]
fn current_subscribers_match_latest_body_template_and_same_callee_output() {
    let mut a = Assembly::default();
    a.call(opcode::STATICCALL, 0x202, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    insert(&mut world, 0x202, &fixture(&[opcode::STOP]), &[]);
    let analysis = run(world);
    evidence("current-deliveries", &analysis);
    assert_current(&analysis);
    let mut checked = 0;
    let mut input_grew = false;
    let mut output_grew = false;
    let mut template_grew = false;
    let mut output_generation_grew = false;
    let mut multiple_subscribers = false;
    for record in analysis.procedure_summaries() {
        input_grew |= record.procedures.iter().any(|p| p.input_updates > 1);
        output_grew |= record.procedures.iter().any(|p| p.output_updates > 1);
        let mut generations = std::collections::BTreeMap::new();
        let mut subscribers = std::collections::BTreeMap::<_, std::collections::BTreeSet<_>>::new();
        for call in &record.regional_cfg.calls {
            subscribers.entry(call.callee).or_default().insert(call.id);
            template_grew |= call.input_revision > 0;
            assert!(!call.deliveries.is_empty());
            for delivery in &call.deliveries {
                output_generation_grew |= delivery.output_generation > 1;
                if let Some(previous) =
                    generations.insert((call.callee, delivery.kind), delivery.output_generation)
                {
                    assert_eq!(
                        previous, delivery.output_generation,
                        "every current subscriber must consume the same latest callee output"
                    );
                }
                checked += 1;
            }
        }
        multiple_subscribers |= subscribers.values().any(|ids| ids.len() > 1);
    }
    assert!(checked >= 2);
    assert!(input_grew && output_grew && template_grew && output_generation_grew);
    assert!(multiple_subscribers);
}

#[test]
fn undeclared_native_only_analyzer_is_rejected_before_process_execution() {
    struct Foreign;
    impl<Cx> Analyzer<Cx> for Foreign {
        type Task = Subject;
        type Error = Infallible;
        fn evaluate(&mut self, _: &Subject, _: &mut Cx) -> Result<LocalClosure, Infallible> {
            Ok(LocalClosure::Stable)
        }
    }
    let mut world = new_world();
    insert(&mut world, A, &fixture(&[opcode::STOP]), &[]);
    let values = native::product();
    let policies = policies();
    let mut foreign = Extension::new(Foreign);
    let analysis = AnalysisEngine::new(&values, &policies, &mut foreign).run(
        AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
        config(),
        &Control::default(),
    );
    assert!(matches!(
        analysis,
        Err(CooperationError::UnsupportedView { .. })
    ));
}

#[test]
fn process_aware_foreign_observer_certifies_real_body_and_return_sources() {
    use analysis_kernel::{Read, Scoped};
    use evm_abstract::{
        analysis::cooperation::{InputRoute, InputView, Inputs},
        domain::NumericValue,
    };
    struct Foreign {
        processes: usize,
    }
    impl<Cx: Read<Inputs<NumericValue>, InputRoute>> Analyzer<Cx> for Foreign {
        type Task = Subject;
        type Error = Cx::Error;
        fn evaluate(
            &mut self,
            subject: &Subject,
            cx: &mut Cx,
        ) -> Result<LocalClosure, Self::Error> {
            let input = cx.query(Scoped::new(subject.scope, ()))?;
            if input
                .value()
                .is_some_and(|input| matches!(input.view(), InputView::Process(_)))
            {
                self.processes += 1;
            }
            Ok(LocalClosure::Stable)
        }
    }
    let mut a = Assembly::default();
    a.call(opcode::STATICCALL, 0x202, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    insert(&mut world, 0x202, &fixture(&[opcode::STOP]), &[]);
    let mut extension = Extension::new(Foreign { processes: 0 }).with_process_view();
    let analysis = AnalysisEngine::new(&native::product(), &policies(), &mut extension)
        .run(
            AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
            config(),
            &Control::default(),
        )
        .unwrap();
    assert!(extension.analyzer.processes > 0);
    assert_current(&analysis);
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .any(|record| !record.regional_cfg.calls.is_empty())
    );
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .flat_map(|record| &record.regional_cfg.calls)
            .flat_map(|call| &call.deliveries)
            .any(|delivery| delivery.current)
    );
}
