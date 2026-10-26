//! Default coverage, explicit switches and native envelopes preserve policy/report semantics.

use super::{Cli, Command, ENTRY};
use crate::{execute::args::RootTerminalCoverageArgs, numeric::NumericModel};
use clap::{CommandFactory, FromArgMatches, Parser, error::ErrorKind};
use evm_abstract::{
    analysis::{EvmAnalysis, EvmWorldAnalysis, ExecutionConfig, Status, control::Control},
    domain::{
        DomainSpec, EvmNumericAlgebra, FiniteSetAlgebra, NumericValue, ProductAlgebra, ValueAlgebra,
    },
    render,
    resource::WorkBudget,
    ssa,
};
use serde_json::Value as Json;

fn assert_metadata(report: &Json, reporting: &WorkBudget) {
    assert_eq!(report["numeric_model"], "product");
    assert_eq!(
        report["postprocessing_max_work"],
        crate::defaults::DEFAULT_POSTPROCESS_WORK
    );
    assert_eq!(report["postprocessing_work"], reporting.used());
    assert!(reporting.used() <= crate::defaults::DEFAULT_POSTPROCESS_WORK);
}

#[test]
fn coverage_defaults_on_and_accepts_only_exclusive_no_value_switches_on_every_input_path() {
    for input in [
        vec!["cfg", "--hex", "00"],
        vec!["cfg", "--file", "unused.hex"],
        vec!["ssa", "--hex", "00"],
        vec!["ssa", "--file", "unused.hex"],
        vec!["analyze", "--world", "unused.json", "--evm.to", ENTRY],
        vec!["analyze", "--rpc", "http://127.0.0.1:1", "--evm.to", ENTRY],
        vec![
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--no-rpc-discovery",
        ],
        vec!["explain", "--hex", "00"],
        vec!["explain", "--file", "unused.hex"],
        vec!["explain", "--world", "unused.json", "--evm.to", ENTRY],
        vec!["explain", "--rpc", "http://127.0.0.1:1", "--evm.to", ENTRY],
        vec![
            "explain",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--no-rpc-discovery",
        ],
    ] {
        for (flag, enabled) in [
            (None, true),
            (Some("--root-terminal-coverage"), true),
            (Some("--no-root-terminal-coverage"), false),
        ] {
            let mut arguments = vec!["evm-abstract"];
            arguments.extend_from_slice(&input);
            if let Some(flag) = flag {
                arguments.push(flag);
            }
            let matches = Cli::command().try_get_matches_from(&arguments).unwrap();
            let matches = matches.subcommand().unwrap().1;
            assert_eq!(
                RootTerminalCoverageArgs::from_arg_matches(matches)
                    .unwrap()
                    .enabled(),
                enabled,
                "{arguments:?}"
            );
            for (field, expected) in [
                ("context_depth", crate::defaults::CONTEXT_DEPTH),
                ("max_states", crate::defaults::MAX_STATES),
                ("max_transfers", crate::defaults::MAX_TRANSFERS),
                ("max_work", crate::defaults::MAX_WORK),
                ("max_call_depth", crate::defaults::MAX_CALL_DEPTH),
                ("max_memory_bytes", crate::defaults::MAX_MEMORY_BYTES),
            ] {
                assert_eq!(
                    matches.get_one::<usize>(field),
                    Some(&expected),
                    "{arguments:?}: {field}"
                );
            }
            assert!(Cli::try_parse_from(&arguments).is_ok());
        }
        for flags in [
            ["--root-terminal-coverage", "--no-root-terminal-coverage"],
            ["--no-root-terminal-coverage", "--root-terminal-coverage"],
        ] {
            let mut arguments = vec!["evm-abstract"];
            arguments.extend_from_slice(&input);
            arguments.extend(flags);
            let error = Cli::try_parse_from(&arguments).err().unwrap();
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{arguments:?}");
        }
        for flag in ["--root-terminal-coverage", "--no-root-terminal-coverage"] {
            for value in ["true", "false"] {
                let assignment = format!("{flag}={value}");
                for flags in [vec![assignment.as_str()], vec![flag, value]] {
                    let mut arguments = vec!["evm-abstract"];
                    arguments.extend_from_slice(&input);
                    arguments.extend(flags);
                    assert!(Cli::try_parse_from(&arguments).is_err(), "{arguments:?}");
                }
            }
        }
    }
    for flag in ["--root-terminal-coverage", "--no-root-terminal-coverage"] {
        assert!(Cli::try_parse_from(["evm-abstract", "disasm", "--hex", "00", flag]).is_err());
    }
}

fn raw(arguments: &[&str], reporting: &mut WorkBudget) -> EvmAnalysis<NumericValue> {
    let parsed = Cli::try_parse_from(arguments).unwrap();
    let (Command::Cfg { args, .. } | Command::Ssa { args, .. }) = parsed.command else {
        panic!("expected raw analysis")
    };
    assert_eq!(args.numeric.numeric_model, NumericModel::Product);
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
    let native = args.analyze(&algebra, &Control::default()).unwrap();
    EvmAnalysis::from_execution(native, reporting).unwrap()
}

fn world(arguments: &[&str]) -> EvmWorldAnalysis<NumericValue> {
    let parsed = Cli::try_parse_from(arguments).unwrap();
    let Command::Analyze { args, .. } = parsed.command else {
        panic!("expected world analysis")
    };
    assert_eq!(args.numeric.numeric_model, NumericModel::Product);
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
    args.analyze(&algebra, &Control::default()).unwrap()
}

fn config(arguments: &[&str]) -> (ExecutionConfig, DomainSpec, NumericModel) {
    fn selected<D: EvmNumericAlgebra>(
        command: Command,
        algebra: &ValueAlgebra<D>,
    ) -> (ExecutionConfig, DomainSpec) {
        match command {
            Command::Analyze { args, .. } => {
                let analysis = args.analyze(algebra, &Control::default()).unwrap();
                (analysis.config().clone(), analysis.domain_spec())
            }
            Command::Cfg { args, .. } | Command::Ssa { args, .. } => {
                let analysis = args.analyze(algebra, &Control::default()).unwrap();
                (analysis.config().clone(), analysis.domain_spec())
            }
            _ => panic!("expected analysis"),
        }
    }
    let command = Cli::try_parse_from(arguments).unwrap().command;
    let numeric = command.numeric().unwrap();
    let spec = numeric.spec().unwrap();
    let model = numeric.numeric_model;
    let (config, spec) = match model {
        NumericModel::Product => {
            selected(command, &ValueAlgebra::new(ProductAlgebra::from_spec(spec)))
        }
        NumericModel::FiniteSet => selected(
            command,
            &ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
        ),
    };
    (config, spec, model)
}

#[test]
fn coverage_switches_reach_the_machine_without_replacing_any_other_config_field() {
    let fixture = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for mut arguments in [
        vec!["evm-abstract", "cfg", "--hex", "00"],
        vec!["evm-abstract", "ssa", "--hex", "00"],
        vec![
            "evm-abstract",
            "analyze",
            "--world",
            &fixture,
            "--evm.to",
            ENTRY,
            "--no-summaries",
        ],
    ] {
        arguments.extend([
            "--max-work",
            "20000000",
            "--max-call-depth",
            "5",
            "--max-memory-bytes",
            "128",
            "--max-states",
            "7",
            "--max-transfers",
            "9",
            "--max-constants",
            "8",
            "--reduction-rounds",
            "2",
            "--max-facts",
            "13",
            "--context-depth",
            "3",
            "--max-symbolic-nodes",
            "256",
            "--max-symbolic-depth",
            "16",
            "--max-relations",
            "24",
            "--numeric-model",
            "finite-set",
            "--no-relations",
        ]);
        let mut expected = config(&arguments);
        assert!(expected.0.root_terminal_coverage);
        let mut enabled = arguments.clone();
        enabled.push("--root-terminal-coverage");
        assert_eq!(
            serde_json::to_vec(&config(&enabled)).unwrap(),
            serde_json::to_vec(&expected).unwrap(),
        );
        expected.0.root_terminal_coverage = false;
        arguments.push("--no-root-terminal-coverage");
        assert_eq!(
            serde_json::to_vec(&config(&arguments)).unwrap(),
            serde_json::to_vec(&expected).unwrap(),
        );
    }
}

#[test]
fn default_coverage_and_the_entire_certificate_survive_existing_ssa_envelopes() {
    let mut reporting = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
    let analysis = raw(
        &[
            "evm-abstract",
            "cfg",
            "--hex",
            "6003565b00",
            "--max-transfers",
            "1",
        ],
        &mut reporting,
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert_eq!(analysis.execution().config().max_work, 2_000_000_000);
    assert!(analysis.execution().config().root_terminal_coverage);
    assert!(analysis.root_terminal_coverage().is_some());
    let native: Json = serde_json::from_slice(&serde_json::to_vec(&analysis).unwrap()).unwrap();
    let machine: Json =
        serde_json::from_slice(&serde_json::to_vec(analysis.execution()).unwrap()).unwrap();
    let projection_work = reporting.used();
    assert!(projection_work > 0);
    let ir = ssa::build_normalized_partial_world(analysis.execution(), &mut reporting).unwrap();
    assert!(reporting.used() > projection_work);
    let forwarded: Json = serde_json::from_str(
        &crate::json::program(&analysis, &ir, NumericModel::Product, &reporting).unwrap(),
    )
    .unwrap();
    assert_eq!(native["schema_version"], 16);
    assert_eq!(machine["schema_version"], 16);
    assert_metadata(&forwarded, &reporting);
    assert_eq!(forwarded["analysis"], native);
    assert_eq!(forwarded["machine_analysis"], machine);
    assert_eq!(
        forwarded["analysis"]["root_terminal_coverage"],
        forwarded["machine_analysis"]["root_terminal_coverage"]
    );
    assert!(forwarded.get("root_terminal_coverage").is_none());
    assert!(forwarded.get("schema_version").is_none());
    assert!(forwarded.get("ssa").is_none());
    assert_eq!(forwarded["partial_ssa"]["frontiers"], machine["frontiers"]);

    let fixture = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let analysis = world(&[
        "evm-abstract",
        "analyze",
        "--world",
        &fixture,
        "--evm.to",
        ENTRY,
        "--max-transfers",
        "1",
    ]);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert_eq!(analysis.config().max_work, 2_000_000_000);
    assert!(analysis.config().root_terminal_coverage);
    assert!(analysis.root_terminal_coverage().is_some());
    let native: Json = serde_json::from_slice(
        &serde_json::to_vec(&render::world::evidence(&analysis).unwrap()).unwrap(),
    )
    .unwrap();
    let mut reporting = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
    let mut plain: Json = serde_json::from_str(
        &crate::json::world(&analysis, None, NumericModel::Product, &reporting).unwrap(),
    )
    .unwrap();
    assert_metadata(&plain, &reporting);
    let ir = ssa::build_normalized_partial_world(&analysis, &mut reporting).unwrap();
    let forwarded: Json = serde_json::from_str(
        &crate::json::world(&analysis, Some(&ir), NumericModel::Product, &reporting).unwrap(),
    )
    .unwrap();
    assert_eq!(native["schema_version"], 16);
    for field in [
        "numeric_model",
        "postprocessing_max_work",
        "postprocessing_work",
    ] {
        plain.as_object_mut().unwrap().remove(field).unwrap();
    }
    assert_eq!(plain, native);
    assert_metadata(&forwarded, &reporting);
    assert_eq!(forwarded["analysis"], native);
    assert!(forwarded.get("root_terminal_coverage").is_none());
    assert!(forwarded.get("schema_version").is_none());
    assert!(forwarded.get("ssa").is_none());
    assert_eq!(forwarded["partial_ssa"]["frontiers"], native["frontiers"]);
}

#[test]
fn explain_forwards_default_enable_and_disable_to_raw_and_world_execution() {
    let fixture = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for input in [
        vec!["--hex", "6003565b00"],
        vec!["--world", &fixture, "--evm.to", ENTRY],
        vec!["--world", &fixture, "--evm.to", ENTRY, "--verbose"],
    ] {
        for (flag, enabled) in [
            (None, true),
            (Some("--root-terminal-coverage"), true),
            (Some("--no-root-terminal-coverage"), false),
        ] {
            let mut arguments = vec!["evm-abstract", "explain", "--max-transfers", "1"];
            arguments.extend_from_slice(&input);
            if let Some(flag) = flag {
                arguments.push(flag);
            }
            let parsed = Cli::try_parse_from(&arguments).unwrap();
            let Command::Explain { args } = parsed.command else {
                panic!("expected explain")
            };
            let algebra =
                ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
            let (text, complete) = args
                .run(
                    &algebra,
                    &Control::default(),
                    &mut WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK),
                )
                .unwrap();
            assert!(!complete);
            assert!(text.contains("Incomplete") && text.contains("Transfers"));
            assert!(text.contains("SSA unavailable"));
            let world_input = input[0] == "--world";
            if world_input {
                assert!(text.contains("Detailed paths: BaselinePending"), "{text}");
                assert!(
                    text.contains(&format!("Native terminal auxiliary requested: {enabled}")),
                    "{text}"
                );
            }
            assert_eq!(
                text.matches("Root terminal coverage (conservative)")
                    .count(),
                usize::from(enabled && !world_input),
                "{text}"
            );
        }
    }
}

#[test]
fn default_enabled_coverage_can_be_absent_when_detail_converges() {
    for command in ["cfg", "ssa"] {
        let mut reporting = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
        let analysis = raw(&["evm-abstract", command, "--hex", "00"], &mut reporting);
        assert_eq!(analysis.status(), Status::Converged);
        assert!(analysis.execution().config().root_terminal_coverage);
        assert!(analysis.root_terminal_coverage().is_none());
        let text = render::cfg(
            &analysis,
            &ValueAlgebra::new(ProductAlgebra::from_spec(
                analysis.execution().domain_spec(),
            )),
        );
        assert_eq!(
            crate::text_with_root_terminal_coverage(
                text.clone(),
                analysis.root_terminal_coverage(),
            ),
            text
        );
    }
}
