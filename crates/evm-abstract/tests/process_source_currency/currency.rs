use crate::{
    A, Assembly, address, assert_current, config, entry, evidence, fixture, insert, native,
    new_world, policies, root_epoch, run,
};
use alloy_primitives::keccak256;
use analysis_kernel::{Analyzer, LocalClosure};
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, FrontierReason, ProcessBoundary, ProcessSource, Status,
        control::Control,
        cooperation::{Extension, Subject},
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
fn enabled_foreign_analyzer_keeps_process_obligations_open() {
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
    let analysis = AnalysisEngine::new(&values, &policies, &mut foreign)
        .run(
            AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
            config(),
            &Control::default(),
        )
        .unwrap();
    evidence("foreign-refusal", &analysis);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.frontiers().iter().any(|f| matches!(
        f.reason,
        FrontierReason::Process(ProcessBoundary::CooperativeInputs)
    )));
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|r| !r.source_current && !r.closed)
    );
}
