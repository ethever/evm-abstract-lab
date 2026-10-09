//! Report identity, rollback resolution and field-level visibility regressions.
use super::*;
use evm_abstract_protocol as api;

fn word(value: &str) -> api::ValueInfo {
    api::ValueInfo {
        summary: value.into(),
        constants: Some(vec![value.into()]),
        known_zero: "0x0".into(),
        known_one: value.into(),
        unsigned: api::WordBounds {
            lower: value.into(),
            upper: value.into(),
        },
        signed: api::WordBounds {
            lower: value.into(),
            upper: value.into(),
        },
        congruence: api::CongruenceValue::Exact(value.into()),
        origins: Some(vec![api::ValueOrigin::Constant]),
        code_address_role: false,
        identity: None,
        expression: None,
        symbolic_limit: false,
    }
}
fn account() -> api::AccountState {
    api::AccountState {
        address: "0x1111111111111111111111111111111111111111".into(),
        existence: api::AccountExistence::Present,
        balance: word("0x0"),
        nonce: word("0x0"),
        code_kind: api::CodeKind::Empty,
        code_hash: None,
        program: None,
        delegation_target: None,
        storage_default: word("0x0"),
        storage: vec![],
        transient_default: word("0x0"),
        transient: vec![],
        created: Some(false),
        pending_destruction: Some(false),
    }
}
fn store() -> api::StoreSnapshot {
    api::StoreSnapshot {
        storage_default: word("0x0"),
        transient_default: word("0x0"),
        balance_default: word("0x0"),
        accounts: vec![account()],
        logs: vec![],
        logs_unknown: false,
    }
}
fn frame(index: usize, rollback_store: usize) -> api::FrameSnapshot {
    let owner = account().address;
    api::FrameSnapshot {
        index,
        program: None,
        code_address: owner.clone(),
        code_hash: "0x00".into(),
        storage_address: owner.clone(),
        address_value: api::AddressValue::Concrete(owner.clone()),
        caller: api::AddressValue::Concrete(owner),
        call_value: word("0x0"),
        is_static: false,
        kind: api::CodeKind::Runtime,
        basic_block: 0,
        context: vec![],
        stack: vec![],
        memory: 0,
        calldata: 0,
        returndata: 0,
        rollback_store,
        continuation: None,
    }
}
#[test]
fn rollback_uses_the_selected_suspended_frame_checkpoint_without_changing_machine_state() {
    let mut stores = vec![store(), store(), store()];
    for (index, store) in stores.iter_mut().enumerate() {
        store.accounts[0].balance = word(&index.to_string());
    }
    let machine = api::MachineSnapshot {
        frames: vec![frame(0, 1), frame(1, 2)],
        store: 0,
    };
    let parent = selected_frame(&machine, Some(0)).unwrap();
    assert_eq!(
        selected_store(&stores, &machine, Some(parent), true)
            .unwrap()
            .accounts[0]
            .balance
            .summary,
        "1"
    );
    let active = selected_frame(&machine, None).unwrap();
    assert_eq!(
        selected_store(&stores, &machine, Some(active), true)
            .unwrap()
            .accounts[0]
            .balance
            .summary,
        "2"
    );
    assert_eq!(
        selected_store(&stores, &machine, Some(parent), false)
            .unwrap()
            .accounts[0]
            .balance
            .summary,
        "0"
    );
    assert_eq!(machine.store, 0);
}
#[test]
fn a_new_report_reusing_s0_cannot_keep_a_historical_exit_or_outcome_selection() {
    let mut workspace = crate::Workspace::default();
    let mut report = crate::tests::report();
    report.states = vec![api::StateDetails {
        state: 0,
        entry: api::MachineSnapshot {
            frames: vec![frame(0, 0)],
            store: 0,
        },
        exit: None,
    }];
    workspace.inspector.point = Point::Exit;
    workspace.inspector.frame = Some(7);
    workspace.inspector.last_state = Some(0);
    workspace.inspector.outcome = 9;
    workspace.inspector.rollback = true;
    workspace.inspector.tab = Tab::Memory;
    workspace.inspector.expanded = false;
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    assert!(
        workspace
            .inspector
            .machine(workspace.report.as_ref().unwrap(), workspace.selection)
            .is_some()
    );
    assert!(workspace.inspector.frame.is_none());
    assert_eq!(workspace.inspector.outcome, 0);
    assert!(!workspace.inspector.rollback);
    assert_eq!(workspace.inspector.tab, Tab::Memory);
    assert!(!workspace.inspector.expanded);
}
#[test]
fn outcome_differences_include_transient_writes_and_explicit_slots_replaced_by_defaults() {
    let mut before = account();
    before.storage.push(api::StorageEntry {
        slot: "0x1".into(),
        value: word("0x2a"),
    });
    let mut after = before.clone();
    after.storage.clear();
    after.transient.push(api::StorageEntry {
        slot: "0x3".into(),
        value: word("0x7"),
    });
    let persistent = stores::changed_slots_for_test(Some(&before), &after, false);
    assert_eq!(persistent.len(), 1);
    assert_eq!(persistent[0].0, "0x1");
    assert_eq!(persistent[0].1.summary, "0x0");
    let transient = stores::changed_slots_for_test(Some(&before), &after, true);
    assert_eq!(transient.len(), 1);
    assert_eq!(transient[0].0, "0x3");
    assert_eq!(transient[0].1.summary, "0x7");
}
#[test]
fn typed_rpc_header_mismatch_renders_both_values_even_when_the_message_omits_them() {
    let error = api::ApiError {
        code: api::ApiErrorCode::Rpc,
        message: "Pinned header changed".into(),
        details: api::ErrorDetails::Rpc(Box::new(api::RpcFailure {
            kind: api::RpcFailureKind::Response,
            chain_id: Some("0x1".into()),
            block_hash: Some("0xfeed".into()),
            method: "eth_getBlockByHash".into(),
            account: None,
            slot: None,
            resource: None,
            limit: None,
            http_status: None,
            rpc_code: None,
            json_line: None,
            json_column: None,
            cause: Some(api::RpcFailureCause::Response(
                api::RpcResponseReason::HeaderChanged(api::HeaderChange {
                    field: api::RpcHeaderField::Timestamp,
                    expected: api::RpcHeaderValue::Quantity("0x1234".into()),
                    observed: api::RpcHeaderValue::Quantity("0x9999".into()),
                }),
            )),
            message: "Pinned header changed".into(),
        })),
    };
    let context = egui::Context::default();
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        },
        |ui| api_error_details(ui, &error),
    );
    fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(item) => text.push(item.galley.text().to_owned()),
            egui::Shape::Vec(items) => {
                for item in items {
                    collect(item, text);
                }
            }
            _ => {}
        }
    }
    output.textures_delta.clear();
    let mut text = Vec::new();
    for shape in output.shapes {
        collect(&shape.shape, &mut text);
    }
    for required in ["Expected", "Observed", "Timestamp", "0x1234", "0x9999"] {
        assert!(
            text.iter().any(|text| text == required),
            "missing {required}: {text:?}"
        );
    }
}

fn rendered(
    context: &egui::Context,
    time: &mut f64,
    events: Vec<egui::Event>,
    contents: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Rect)> {
    *time += 0.1;
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(520.0, 240.0),
            )),
            time: Some(*time),
            events,
            ..Default::default()
        },
        contents,
    );
    fn collect(shape: &egui::Shape, clip: egui::Rect, text: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(item) => {
                let rect = egui::Rect::from_min_size(item.pos, item.galley.size());
                if clip.intersects(rect) {
                    text.push((item.galley.text().to_owned(), rect.intersect(clip)));
                }
            }
            egui::Shape::Vec(items) => {
                for item in items {
                    collect(item, clip, text);
                }
            }
            _ => {}
        }
    }
    let mut text = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut text);
    }
    output.textures_delta.clear();
    text
}
fn click(
    context: &egui::Context,
    time: &mut f64,
    position: egui::Pos2,
    mut contents: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Rect)> {
    for pressed in [true, false] {
        rendered(
            context,
            time,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut contents,
        );
    }
    rendered(context, time, vec![], contents)
}
#[test]
fn many_outcomes_keep_details_visible_and_the_last_exact_result_selectable() {
    let mut report = crate::tests::report();
    report.stores = vec![store()];
    report.byte_arrays = (0..64)
        .map(|index| api::ByteArraySnapshot {
            length: word(&format!("0x{index:x}")),
            default: word("0x0"),
            cells: vec![],
            values: vec![],
            memory: false,
        })
        .collect();
    report.outcomes = (0..64)
        .map(|index| api::Outcome {
            state: report.cfg[index % report.cfg.len()].id,
            kind: api::OutcomeKind::Return,
            data: index,
            store: 0,
        })
        .collect();
    let context = egui::Context::default();
    context.global_style_mut(|style| style.animation_time = 0.0);
    let mut time = 0.0;
    let mut selection = Selection::default();
    let mut inspector = Inspector {
        tab: Tab::Outcomes,
        ..Inspector::default()
    };
    let text = rendered(&context, &mut time, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    assert!(
        text.iter().any(|(text, _)| text == "Return/revert length"),
        "outcomes must not fill the whole short panel"
    );
    let position = text
        .iter()
        .find(|(text, _)| text.starts_with("Outcome 1 / 64"))
        .unwrap()
        .1
        .center();
    let opened = click(&context, &mut time, position, |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let menu = opened
        .iter()
        .find(|(text, _)| text.starts_with("Outcome 1 ·"))
        .unwrap()
        .1
        .center();
    let mut visible = rendered(
        &context,
        &mut time,
        vec![
            egui::Event::PointerMoved(menu),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -3000.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
        |ui| inspector.show(ui, &report, &mut selection, None),
    );
    for _ in 0..30 {
        visible = rendered(&context, &mut time, vec![], |ui| {
            inspector.show(ui, &report, &mut selection, None)
        });
    }
    let last = visible
        .iter()
        .find(|(text, _)| text.starts_with("Outcome 64 ·"))
        .expect("last outcome is reachable by scrolling the chooser")
        .1
        .center();
    let selected = click(&context, &mut time, last, |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    assert_eq!(inspector.outcome, 63);
    assert_eq!(selection.state, Some(report.outcomes[63].state));
    assert!(
        selected.iter().any(|(text, _)| text == "0x3f"),
        "show the selected outcome's bytes, independent of active state"
    );
}
#[test]
fn expanded_account_facts_leave_storage_cells_visible_in_a_short_area() {
    let mut report = crate::tests::report();
    let mut initial = account();
    initial.storage = (0..32)
        .map(|index| api::StorageEntry {
            slot: format!("0x{:x}", 0x100 + index),
            value: word("0xcafe"),
        })
        .collect();
    let address = initial.address.clone();
    report.accounts = vec![initial];
    let context = egui::Context::default();
    context.global_style_mut(|style| style.animation_time = 0.0);
    let mut time = 0.0;
    let text = rendered(&context, &mut time, vec![], |ui| {
        ui.set_max_height(180.0);
        stores::storage(ui, &report, None, &address, false);
    });
    let position = text
        .iter()
        .find(|(text, _)| text == "Account facts in this store")
        .unwrap()
        .1
        .center();
    let expanded = click(&context, &mut time, position, |ui| {
        ui.set_max_height(180.0);
        stores::storage(ui, &report, None, &address, false);
    });
    assert!(expanded.iter().any(|(text, _)| text == "Balance"));
    assert!(
        expanded
            .iter()
            .any(|(text, rect)| text == "0x100" && rect.bottom() <= 180.0),
        "slot list stays reachable below bounded facts: {expanded:?}"
    );
}

#[test]
fn relational_frontier_details_show_fact_arity_and_distinguish_solver_failure() {
    let mut report = crate::tests::report();
    report.diagnostics.clear();
    report.frontiers = vec![api::Frontier {
        from: Some(0),
        pc: Some(4),
        kind: api::FrontierKind::Relations,
        detail: "not the authoritative cause".into(),
        reason: api::FrontierDetails::Relations(api::QueryBoundary::ScalarFactError(
            api::FactFailure::InvalidOperation(api::FactOperationFailure {
                opcode: 0x16,
                expected: 2,
                actual: 1,
            }),
        )),
    }];
    let context = egui::Context::default();
    let mut time = 0.0;
    let mut selection = Selection::default();
    let arity = rendered(&context, &mut time, vec![], |ui| {
        evidence::diagnostics(ui, &report, &mut selection)
    });
    for required in [
        "Pure-operation operand count mismatch",
        "0x16",
        "Expected operands",
        "2",
        "Actual operands",
        "1",
    ] {
        assert!(
            arity.iter().any(|(text, _)| text == required),
            "missing {required}: {arity:?}"
        );
    }
    report.frontiers[0].reason =
        api::FrontierDetails::Relations(api::QueryBoundary::SolverError(api::SolverFailure {
            provider: api::SmtProvider::Cvc5,
            message: "binding rejected term".into(),
        }));
    let failure = rendered(&context, &mut time, vec![], |ui| {
        evidence::diagnostics(ui, &report, &mut selection)
    });
    for required in [
        "Solver binding or encoding failed",
        "Provider",
        "Cvc5",
        "Binding diagnostic",
        "binding rejected term",
    ] {
        assert!(
            failure.iter().any(|(text, _)| text == required),
            "missing {required}: {failure:?}"
        );
    }
    assert!(
        !failure
            .iter()
            .any(|(text, _)| text == "Solver could not decide")
    );
}
