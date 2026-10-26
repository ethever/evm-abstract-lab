use super::Stream;
use crate::{hybrid::WorldAnalysis, numeric::NumericModel};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, HybridConfig,
        control::Control, cooperation::NoExtension,
    },
    bytecode::Program,
    domain::{ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

#[test]
fn staged_stream_flushes_complete_closed_global_line_before_native_then_final() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let mut output = Vec::new();
    let mut budget = WorkBudget::new(10_000_000);
    let mut stream = Stream::new(&mut output, &mut budget, NumericModel::Product, &control);
    let input = AnalysisInput::from_program(
        Program::from_hex("600160020100").unwrap(),
        Default::default(),
    )
    .unwrap();
    let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            input,
            ExecutionConfig::default(),
            HybridConfig::default(),
            &control,
            &mut stream,
        )
        .unwrap();
    let early: serde_json::Value = serde_json::from_slice(stream.writer).unwrap();
    assert_eq!(early["lifecycle"], "Running");
    assert_eq!(early["model_closed"], true);
    assert_eq!(early["report"]["coverage"]["work"]["preparation"], 0);
    assert_eq!(early["report"]["coverage"]["work"]["detail"], 0);
    assert!(early["report"]["detail"].is_null());
    assert!(report.detail().is_some());
    let first_work = stream.budget.used();
    stream.finish(&WorldAnalysis::from(report)).unwrap();
    let lines: Vec<_> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let final_line: serde_json::Value = serde_json::from_slice(lines[1]).unwrap();
    assert_eq!(final_line["lifecycle"], "Final");
    assert_eq!(final_line["version"], 2);
    assert_eq!(final_line["global_version"], 1);
    assert_eq!(early["snapshot"], final_line["snapshot"]);
    assert_eq!(early["revision"], final_line["revision"]);
    assert!(budget.used() > first_work);
}

#[test]
fn final_export_refusal_keeps_the_flushed_baseline_and_never_resets_its_payer() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let mut output = Vec::new();
    let mut budget = WorkBudget::new(10_000_000);
    let mut stream = Stream::new(&mut output, &mut budget, NumericModel::Product, &control);
    let input =
        AnalysisInput::from_program(Program::from_hex("00").unwrap(), Default::default()).unwrap();
    let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            input,
            ExecutionConfig::default(),
            HybridConfig::default(),
            &control,
            &mut stream,
        )
        .unwrap();
    let before = stream.writer.clone();
    assert!(stream.budget.charge(10_000_000 - stream.budget.used()));
    assert!(stream.finish(&WorldAnalysis::from(report)).is_err());
    assert_eq!(output, before);
    assert_eq!(budget.used(), 10_000_000);
    let baseline: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(baseline["model_closed"], true);
}
