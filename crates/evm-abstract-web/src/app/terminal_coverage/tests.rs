//! Native terminal attachments survive the wire and frontend boundaries.
//! Fixtures use the native producer; no display code mints a certificate.

use super::super::{Command, JobOperation, Message, Selection, Workspace, inspector};
use super::*;
use egui::{Context, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol as api;

pub(crate) const TWO_BLOCK_RETURN: &str = "6003565b602a5f5260205ff3";

#[test]
fn terminal_binding_checks_the_selected_model_and_actual_numeric_limits() {
    for model in [api::NumericModel::Product, api::NumericModel::FiniteSet] {
        let request = api::AnalyzeRequest {
            input: api::AnalysisInput::Bytecode(api::BytecodeInput {
                bytecode: TWO_BLOCK_RETURN.into(),
                address: None,
            }),
            limits: api::AnalysisLimits {
                numeric_model: model,
                max_transfers: 1,
                ..api::AnalysisLimits::default()
            },
            ..api::AnalyzeRequest::default()
        };
        let valid = evm_abstract_server::analyze::analyze(request).unwrap();
        let coverage = valid.root_terminal_coverage.as_ref().unwrap();
        assert_eq!(
            valid.validate(),
            Ok(()),
            "{model:?}: failure data length {:?}",
            valid.byte_arrays[coverage.outcomes[2].data].length
        );
        let input = &valid.root_terminal_coverage.as_ref().unwrap().input;
        assert_eq!(input.numeric_model, model);
        assert_eq!(
            input.domain_spec.capacity,
            valid.metadata.limits.max_constants
        );
        for field in 0..9 {
            let mut invalid = valid.clone();
            let input = &mut invalid.root_terminal_coverage.as_mut().unwrap().input;
            match field {
                0 => input.domain_spec.capacity += 1,
                1 => input.domain_spec.reduction_rounds += 1,
                2 => input.domain_spec.fact_limit += 1,
                3 => input.domain_spec.relations.enabled = !input.domain_spec.relations.enabled,
                4 => input.domain_spec.relations.max_constraints += 1,
                5 => input.domain_spec.relations.max_nodes += 1,
                6 => input.domain_spec.relations.max_depth += 1,
                7 => {
                    input.numeric_model = match model {
                        api::NumericModel::Product => api::NumericModel::FiniteSet,
                        api::NumericModel::FiniteSet => api::NumericModel::Product,
                    };
                }
                _ => input.config.max_work += 1,
            }
            let expected = if field < 7 {
                api::RootTerminalValidationFailure::DomainSpec
            } else {
                api::RootTerminalValidationFailure::Configuration
            };
            assert_eq!(
                invalid.validate(),
                Err(api::ReportValidationFailure::RootTerminal(expected)),
                "{model:?} field {field}"
            );
        }
    }
}

pub(crate) fn report() -> AnalysisReport {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: TWO_BLOCK_RETURN.into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
    request.limits.max_transfers = 1;
    request.limits.root_terminal_coverage = true;
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
    // The gas envelope retains entry Failure even before terminal bytecode runs.
    assert!(!report.outcomes.is_empty());
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::Transfers)
    );
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    assert_eq!(coverage.scope, api::RootTerminalScope::AllInputStates);
    assert_eq!(coverage.trigger, api::RootTerminalTrigger::Transfers);
    assert_eq!(
        coverage.outcomes.each_ref().map(|outcome| outcome.kind),
        [
            api::OutcomeKind::Return,
            api::OutcomeKind::Revert,
            api::OutcomeKind::Failure
        ]
    );
    let entry = &report.states[0];
    let root = &entry.entry.frames[0];
    assert_eq!(coverage.input.saved_store, root.rollback_store);
    assert_eq!(coverage.input.initial_store, entry.entry.store);
    assert_eq!(coverage.input.program, root.program.unwrap());
    let saved = &report.stores[root.rollback_store];
    let api::Expression::SlotsInput(source) = &report.symbolic.nodes[saved.storage_root.node]
    else {
        panic!("expected original persistent storage source");
    };
    assert_eq!(source.source.token, coverage.input.storage_source);
    assert_eq!(source.source.namespace, "0x0");
    assert!(source.bindings.is_empty());
    let [known] = saved.accounts.as_slice() else {
        panic!("expected one saved runtime account");
    };
    assert_eq!(known.address, coverage.input.entry_address);
    assert_eq!(known.code_kind, api::CodeKind::Runtime);
    assert_eq!(known.program, root.program);
    assert_eq!(known.code_hash, root.code_hash);
    assert_eq!(
        report.programs[coverage.input.program].bytecode,
        format!("0x{TWO_BLOCK_RETURN}")
    );
    let mut owner_source = source.clone();
    owner_source.source.namespace = format!(
        "0x{}00",
        known
            .address
            .trim_start_matches("0x")
            .trim_start_matches('0')
    );
    assert_eq!(
        report.symbolic.nodes[known.storage_root.node],
        api::Expression::SlotsInput(owner_source)
    );
    let mut pre_entry_failures = 0;
    let mut late_failures = 0;
    for outcome in &report.outcomes {
        assert_eq!(outcome.kind, api::OutcomeKind::Failure);
        assert_eq!(outcome.state, entry.state);
        let data = &report.byte_arrays[outcome.data];
        assert_eq!(data.length.unsigned.lower, "0x0");
        assert_eq!(data.length.unsigned.upper, "0x0");
        assert!(data.cells.is_empty());
        assert!(data.values.is_empty());
        assert!(!data.memory);
        assert_eq!(data, &report.byte_arrays[coverage.outcomes[2].data]);
        let store = &report.stores[outcome.store];
        // Engine's pre-entry Failure clones the whole saved Store before any
        // sticky owner-3 touch or transaction finalization.
        if store == saved {
            pre_entry_failures += 1;
            continue;
        }
        // Unknown To may touch owner 3; restore retains that sticky touch.
        // Finalization joins its original storage with deletion under its own
        // guard, independently of the prepared future Failure's finalization.
        let ripemd = "0x0000000000000000000000000000000000000003";
        let cleanup = store
            .accounts
            .iter()
            .find(|row| row.address == ripemd)
            .unwrap();
        let api::Expression::SlotsIte(deletion) = &report.symbolic.nodes[cleanup.storage_root.node]
        else {
            panic!("expected owner-3 guarded cleanup");
        };
        let guard = &report.symbolic.nodes[deletion.condition.node];
        assert!(matches!(guard, api::Expression::Fresh(_)));
        assert_ne!(
            guard,
            &report.symbolic.nodes[coverage.input.storage_source.node]
        );
        let mut original = source.clone();
        original.source.namespace = "0x300".into();
        assert_eq!(
            report.symbolic.nodes[deletion.then_state.node],
            api::Expression::SlotsInput(original)
        );
        assert_eq!(
            report.symbolic.nodes[deletion.else_state.node],
            api::Expression::SlotsZero
        );
        for index in [
            coverage.input.environment.calldata,
            coverage.input.calldata,
            coverage.outcomes[0].data,
            coverage.outcomes[1].data,
        ] {
            let api::Expression::View(view) =
                &report.symbolic.nodes[report.byte_arrays[index].symbolic.node]
            else {
                panic!("expected fixture byte view");
            };
            let api::Expression::BytesInput(input) = &report.symbolic.nodes[view.state.node] else {
                panic!("expected independent fixture byte source");
            };
            for token in [view.length, input.source.token] {
                assert_ne!(guard, &report.symbolic.nodes[token.node]);
            }
        }
        let mut expected = saved.clone();
        expected.created_default = Some(false);
        expected.pending_destruction_default = Some(false);
        expected.touched_default = Some(false);
        expected.transient_facts.clear();
        for account in &mut expected.accounts {
            account.created = Some(false);
            account.pending_destruction = Some(false);
            account.touched = Some(false);
            account.transient_default = saved.transient_default.clone();
            account.transient_root = saved.transient_root;
            account.transient.clear();
        }
        expected.accounts.insert(
            0,
            api::AccountState {
                address: ripemd.into(),
                existence: api::AccountExistence::Unknown,
                balance: saved.balance_default.clone(),
                nonce: saved.nonce_default.clone(),
                code_kind: api::CodeKind::Unknown,
                code_hash: None,
                program: None,
                delegation_target: None,
                storage_default: saved.storage_default.clone(),
                storage_root: cleanup.storage_root,
                transient_root: saved.transient_root,
                storage: vec![],
                transient_default: saved.transient_default.clone(),
                transient: vec![],
                created: Some(false),
                touched: Some(false),
                pending_destruction: Some(false),
            },
        );
        // Preserve complete values, all accounts, logs, facts and source IDs.
        assert_eq!(store, &expected);
        assert_eq!(store.created_default, Some(false));
        assert_eq!(store.pending_destruction_default, Some(false));
        assert_eq!(store.touched_default, Some(false));
        assert_eq!(store.transient_default.unsigned.lower, "0x0");
        assert_eq!(store.transient_default.unsigned.upper, "0x0");
        assert_eq!(
            report.symbolic.nodes[store.transient_root.node],
            api::Expression::SlotsZero
        );
        assert!(store.transient_facts.is_empty());
        for account in &store.accounts {
            assert_eq!(account.created, Some(false));
            assert_eq!(account.pending_destruction, Some(false));
            assert_eq!(account.touched, Some(false));
            assert!(account.transient.is_empty());
            assert_eq!(account.transient_default.unsigned.lower, "0x0");
            assert_eq!(account.transient_default.unsigned.upper, "0x0");
            assert_eq!(
                report.symbolic.nodes[account.transient_root.node],
                api::Expression::SlotsZero
            );
        }
        late_failures += 1;
    }
    assert!(pre_entry_failures > 0);
    assert!(late_failures > 0);
    assert_eq!(pre_entry_failures + late_failures, report.outcomes.len());
    let wire = serde_json::to_vec(&api::AnalyzeReply {
        result: Ok(report.clone()),
    })
    .unwrap();
    let parsed: api::AnalyzeReply = serde_json::from_slice(&wire).unwrap();
    let parsed = parsed.result.unwrap();
    assert_eq!(parsed, report);
    parsed.validate().unwrap();
    parsed
}

/// Exercise opaque identity width independently of the native process counter.
/// All occurrences of the single original input namespace are renamed together.
pub(crate) fn full_width_report() -> AnalysisReport {
    let mut report = report();
    let input = &mut report.root_terminal_coverage.as_mut().unwrap().input;
    let old_scope = input.input_scope.unwrap().0;
    input.input_scope = Some(api::DecimalU64(u64::MAX));
    input.revision = api::DecimalU64(u64::MAX - 1);
    let api::RootTerminalSnapshot::Offline(snapshot) = &mut input.snapshot else {
        panic!("expected offline source");
    };
    snapshot.namespace = api::DecimalU64(u64::MAX);
    let wire = serde_json::to_string(&report).unwrap().replace(
        &format!("\"scope\":\"{old_scope}\""),
        &format!("\"scope\":\"{}\"", u64::MAX),
    );
    let report: AnalysisReport = serde_json::from_str(&wire).unwrap();
    report.validate().unwrap();
    report
}

fn draw(context: &Context, workspace: &mut Workspace) -> FullOutput {
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1100.0))),
            ..RawInput::default()
        },
        |ui| {
            assert!(workspace.show(ui).is_none());
        },
    );
    output.textures_delta.clear();
    output
}

fn text(output: &FullOutput) -> Vec<String> {
    fn collect(shape: &egui::Shape, clip: Rect, labels: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                if clip.intersects(rect) {
                    labels.push(evm_abstract_notation::normalize_subscripts(
                        text.galley.text(),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, labels);
                }
            }
            _ => {}
        }
    }
    let mut labels = vec![];
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut labels);
    }
    labels
}

#[test]
fn explicit_off_is_quiet_and_requested_none_reports_only_incomplete_absence() {
    let context = Context::default();
    crate::palette::configure(&context);
    let mut workspace = Workspace::default();
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    let ordinary = evm_abstract_server::analyze::analyze(request.clone()).unwrap();
    ordinary.validate().unwrap();
    assert!(!ordinary.metadata.limits.root_terminal_coverage);
    assert!(ordinary.root_terminal_coverage.is_none());
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(ordinary.clone()),
    }));
    draw(&context, &mut workspace);
    assert_eq!(status(&ordinary), None);
    assert!(!workspace.accessible_status().contains("Root terminal"));
    assert!(
        !text(&draw(&context, &mut workspace))
            .iter()
            .any(|text| text.contains("terminal coverage"))
    );
    assert_eq!(workspace.report.as_ref().unwrap(), &ordinary);
    request.limits.root_terminal_coverage = true;
    let ordinary = evm_abstract_server::analyze::analyze(request.clone()).unwrap();
    ordinary.validate().unwrap();
    assert!(ordinary.metadata.limits.root_terminal_coverage);
    assert_eq!(ordinary.metadata.limits.max_work, 2_000_000_000);
    assert_eq!(ordinary.status, api::AnalysisStatus::Converged);
    assert!(ordinary.ssa.complete);
    assert!(ordinary.frontiers.is_empty());
    assert!(ordinary.root_terminal_coverage.is_none());
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(ordinary.clone()),
    }));
    draw(&context, &mut workspace);
    let painted = text(&draw(&context, &mut workspace));
    assert_eq!(status(&ordinary), None);
    assert!(!painted.iter().any(|text| text == COVERED));
    assert!(!painted.iter().any(|text| text == NOT_ATTACHED));
    assert!(!workspace.accessible_status().contains(NOT_ATTACHED));
    assert!(!workspace.accessible_status().contains(COVERED));
    assert!(!workspace.accessible_status().contains("budget exhausted"));
    assert!(!workspace.accessible_status().contains("Experimental"));
    assert_eq!(workspace.report.as_ref().unwrap(), &ordinary);

    request.limits.max_work = 1;
    let incomplete = evm_abstract_server::analyze::analyze(request).unwrap();
    incomplete.validate().unwrap();
    assert!(incomplete.metadata.limits.root_terminal_coverage);
    assert_eq!(incomplete.metadata.limits.max_work, 1);
    assert_eq!(incomplete.status, api::AnalysisStatus::Incomplete);
    assert!(!incomplete.ssa.complete);
    assert!(!incomplete.frontiers.is_empty());
    assert!(incomplete.root_terminal_coverage.is_none());
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(incomplete.clone()),
    }));
    draw(&context, &mut workspace);
    let painted = text(&draw(&context, &mut workspace));
    assert_eq!(status(&incomplete), Some(NOT_ATTACHED));
    assert!(painted.iter().any(|text| text == NOT_ATTACHED));
    assert!(!painted.iter().any(|text| text == COVERED));
    assert!(workspace.accessible_status().contains(NOT_ATTACHED));
    assert!(!workspace.accessible_status().contains(COVERED));
    assert_eq!(workspace.report.as_ref().unwrap(), &incomplete);
}

#[test]
fn attached_native_relation_keeps_partial_detail_and_actual_counts_visible() {
    let report = report();
    let detailed = (
        report.cfg.clone(),
        report.ssa.clone(),
        report.frontiers.clone(),
        report.states.clone(),
        report.pending_machines.clone(),
        report.outcomes.clone(),
    );
    let mut workspace = Workspace::default();
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(report.clone()),
    }));
    let context = Context::default();
    crate::palette::configure(&context);
    draw(&context, &mut workspace);
    let painted = text(&draw(&context, &mut workspace));
    for expected in ["Detail incomplete / partial SSA", COVERED] {
        assert!(
            painted.iter().any(|text| text == expected),
            "{expected}: {painted:?}"
        );
        assert!(workspace.accessible_status().contains(expected));
    }
    let live = workspace.accessible_status();
    assert!(live.contains("Incomplete; SSA partial"));
    assert!(live.contains(&format!("{} outcomes", report.outcomes.len())));
    assert!(!live.contains("Converged"));
    assert!(!painted.iter().any(|text| text == "Model converged"));
    let installed = workspace.report.as_ref().unwrap();
    assert_eq!(
        (
            installed.cfg.clone(),
            installed.ssa.clone(),
            installed.frontiers.clone(),
            installed.states.clone(),
            installed.pending_machines.clone(),
            installed.outcomes.clone()
        ),
        detailed
    );
    assert_eq!(installed, &report);
    workspace.inspector.tab = inspector::Tab::Diagnostics;
    assert!(
        workspace
            .inspector
            .accessible_summary(installed, workspace.selection)
            .contains(&format!("{} unexpanded frontiers", report.frontiers.len()))
    );
    workspace.inspector.tab = inspector::Tab::Outcomes;
    let live = workspace
        .inspector
        .accessible_summary(installed, workspace.selection);
    for expected in [
        "Return alternative",
        "Revert alternative",
        "Failure alternative",
    ] {
        assert!(live.contains(expected), "{live}");
    }
    assert!(live.contains(&format!("{} observed outcomes", report.outcomes.len())));
}

#[test]
fn invalid_schema_bindings_and_pool_sorts_never_replace_the_visible_report() {
    let valid = report();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(valid.clone()),
    }));
    workspace.selection = Selection {
        state: Some(valid.cfg[0].id),
        pc: Some(0),
        value: None,
    };
    workspace.account = Some(valid.metadata.entry_address.clone());
    workspace.inspector.tab = inspector::Tab::Outcomes;
    let before_selection = workspace.selection;
    let before_account = workspace.account.clone();
    let before_program = workspace.program;
    let before_graph = workspace.graph.accessible_summary();
    let failure_data = valid.root_terminal_coverage.as_ref().unwrap().outcomes[2].data;
    for (constants, lower, upper, accepted) in [
        (None, "0x00".to_owned(), "0x0".to_owned(), true),
        (
            Some(vec!["0x0000".into()]),
            "0x0".into(),
            format!("0x{}", "f".repeat(64)),
            true,
        ),
        (
            Some(vec!["0x0".into(), "0x1".into()]),
            "0x0".into(),
            format!("0x{}", "f".repeat(64)),
            false,
        ),
        (None, "0x0".into(), format!("0x{}", "f".repeat(64)), false),
        (Some(vec!["0x1".into()]), "0x1".into(), "0x1".into(), false),
        (Some(vec![]), "0x0".into(), "0x1".into(), false),
        (Some(vec!["0x".into()]), "0x0".into(), "0x1".into(), false),
    ] {
        let mut projected = valid.clone();
        let length = &mut projected.byte_arrays[failure_data].length;
        length.constants = constants;
        length.unsigned = api::WordBounds { lower, upper };
        length.known_zero = "0x0".into();
        length.known_one = "0x0".into();
        length.signed = api::WordBounds {
            lower: format!("0x8{}", "0".repeat(63)),
            upper: format!("0x7{}", "f".repeat(63)),
        };
        length.congruence = api::CongruenceValue::Any;
        length.nonzero = false;
        if accepted {
            projected.validate().unwrap();
            workspace.receive(Ok(api::AnalyzeReply {
                result: Ok(projected.clone()),
            }));
            assert_eq!(workspace.report.as_ref(), Some(&projected));
            workspace.receive(Ok(api::AnalyzeReply {
                result: Ok(valid.clone()),
            }));
            workspace.selection = before_selection;
            workspace.account.clone_from(&before_account);
            workspace.inspector.tab = inspector::Tab::Outcomes;
        } else {
            assert_eq!(
                projected.validate(),
                Err(api::ReportValidationFailure::RootTerminal(
                    api::RootTerminalValidationFailure::FailureData
                ))
            );
            workspace.receive(Ok(api::AnalyzeReply {
                result: Ok(projected),
            }));
            assert_eq!(workspace.report.as_ref(), Some(&valid));
            assert_eq!(workspace.selection, before_selection);
            assert_eq!(workspace.account, before_account);
            assert_eq!(workspace.program, before_program);
            assert_eq!(workspace.graph.accessible_summary(), before_graph);
            assert!(workspace.inspector.tab == inspector::Tab::Outcomes);
        }
    }
    let mut invalid_reports = vec![];
    for version in [api::SCHEMA_VERSION - 1, api::SCHEMA_VERSION + 1] {
        let mut invalid = valid.clone();
        invalid.schema_version = version;
        invalid_reports.push(invalid);
    }
    for which in 0..8 {
        let mut invalid = valid.clone();
        let coverage = invalid.root_terminal_coverage.as_mut().unwrap();
        match which {
            0 => coverage.outcomes[0].data = invalid.byte_arrays.len(),
            1 => coverage.outcomes[1].store = invalid.stores.len(),
            2 => coverage.input.saved_store = invalid.stores.len(),
            3 => coverage.input.program = usize::MAX,
            4 => coverage.input.storage_source.node = invalid.stores[0].storage_root.node,
            5 => coverage.outcomes.swap(0, 1),
            6 => coverage.input.config.max_work += 1,
            _ => invalid.stores[0].transient_root.node = coverage.input.storage_source.node,
        }
        assert!(
            invalid.validate().is_err(),
            "invalid case {which} was accepted"
        );
        invalid_reports.push(invalid);
    }
    for invalid in invalid_reports {
        workspace.receive(Ok(api::AnalyzeReply {
            result: Ok(invalid),
        }));
        assert_eq!(workspace.report.as_ref().unwrap(), &valid);
        assert_eq!(workspace.selection, before_selection);
        assert_eq!(workspace.account, before_account);
        assert_eq!(workspace.program, before_program);
        assert_eq!(workspace.graph.accessible_summary(), before_graph);
        assert!(workspace.inspector.tab == inspector::Tab::Outcomes);
    }
}

#[test]
fn malformed_identity_and_new_wire_fields_are_rejected_before_installation() {
    let report = full_width_report();
    let wire = serde_json::to_string(&api::AnalyzeReply {
        result: Ok(report.clone()),
    })
    .unwrap();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(serde_json::from_str(&wire).unwrap()));
    let selection = workspace.selection;
    for malformed in [
        "\"01\"",
        "\"18446744073709551616\"",
        "18446744073709551615",
        "\"-1\"",
    ] {
        let invalid = wire.replace(
            "\"namespace\":\"18446744073709551615\"",
            &format!("\"namespace\":{malformed}"),
        );
        assert_ne!(invalid, wire);
        let cause = serde_json::from_str::<api::AnalyzeReply>(&invalid).unwrap_err();
        workspace.receive(Err(super::super::TransportError::Decode {
            status: 200,
            cause,
        }));
        assert_eq!(workspace.report.as_ref().unwrap(), &report);
        assert_eq!(workspace.selection, selection);
    }
    for (needle, replacement) in [
        ("\"scope\":\"18446744073709551615\"", "\"scope\":\"00\""),
        ("\"root_terminal_coverage\":null,", ""),
        ("\"nonce_default\":", "\"unrecognized_default\":"),
    ] {
        let source = if needle.contains("null") {
            let mut absent = report.clone();
            absent.root_terminal_coverage = None;
            serde_json::to_string(&api::AnalyzeReply { result: Ok(absent) }).unwrap()
        } else {
            wire.clone()
        };
        let invalid = source.replace(needle, replacement);
        assert_ne!(invalid, source);
        let cause = serde_json::from_str::<api::AnalyzeReply>(&invalid).unwrap_err();
        workspace.receive(Err(super::super::TransportError::Decode {
            status: 200,
            cause,
        }));
        assert_eq!(workspace.report.as_ref().unwrap(), &report);
        assert_eq!(workspace.selection, selection);
    }
}

#[test]
fn stale_coverage_reply_cannot_replace_a_newer_generation() {
    let mut workspace = Workspace::default();
    let Command::Submit {
        generation: old, ..
    } = workspace.initial_command()
    else {
        panic!("expected submit")
    };
    let Command::Submit {
        generation: current,
        ..
    } = workspace.initial_command()
    else {
        panic!("expected submit")
    };
    assert_ne!(old, current);
    let retained = crate::tests::report();
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(retained.clone()),
    }));
    let selection = workspace.selection;
    workspace.receive_message(Message::Result {
        generation: old,
        id: api::JobId(u64::MAX),
        result: Box::new(Ok(api::AnalyzeReply {
            result: Ok(report()),
        })),
        transfer_ns: 0,
        parsing_ns: 0,
    });
    assert_eq!(workspace.report.as_ref().unwrap(), &retained);
    assert_eq!(workspace.selection, selection);
    assert!(!workspace.accessible_status().contains(COVERED));
}

#[test]
fn inspecting_coverage_keeps_the_job_lifecycle_and_selection_unchanged() {
    let mut workspace = Workspace::default();
    let Command::Submit { generation, .. } = workspace.initial_command() else {
        panic!("expected submit")
    };
    workspace.receive_message(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Ok(api::JobReply {
            result: Ok(api::JobSnapshot {
                id: api::JobId(u64::MAX),
                state: api::JobState::Completed,
                progress: api::AnalysisProgress::default(),
                timings: api::AnalysisTimings::default(),
            }),
        }),
    });
    workspace.receive_message(Message::Result {
        generation,
        id: api::JobId(u64::MAX),
        result: Box::new(Ok(api::AnalyzeReply {
            result: Ok(report()),
        })),
        transfer_ns: 0,
        parsing_ns: 0,
    });
    let selection = workspace.selection;
    workspace.inspector.tab = inspector::Tab::Outcomes;
    let context = Context::default();
    crate::palette::configure(&context);
    for _ in 0..3 {
        draw(&context, &mut workspace);
        assert_eq!(workspace.task.generation(), generation);
    }
    assert_eq!(workspace.selection, selection);
    assert!(
        workspace
            .accessible_status()
            .contains("18446744073709551615")
    );
}
