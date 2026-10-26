//! Display counterexamples distinguish supplied rows; no execution precision is
//! claimed by the deliberately distinct balance/slot observations below.

use super::*;
use std::sync::Arc;

const PHYSICAL: &str = "0x2222222222222222222222222222222222222222";
const SLOT: &str = "0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe";

fn original_report() -> api::AnalysisReport {
    let mut report = report().clone();
    report.native = None;
    report.global = None;
    report.selected_view = api::AnalysisNamespace::Baseline;
    report.rich_export = api::RichExportOutcome::NotRequested;
    report.hybrid.as_mut().unwrap().local_prefixes.clear();
    let model = Arc::make_mut(report.minimum.as_mut().unwrap());
    let root = model
        .accounts
        .iter_mut()
        .find(|row| row.root_template)
        .unwrap();
    root.balance = word("0xa1");
    root.storage = vec![api::StorageEntry {
        slot: SLOT.into(),
        value: word("0xc3"),
    }];
    let mut physical = root.clone();
    physical.address = PHYSICAL.into();
    physical.root_template = false;
    physical.program = None;
    physical.code_kind = api::CodeKind::Empty;
    physical.balance = word("0xb2");
    physical.storage[0].value = word("0xd4");
    // Root resolution must follow the binding, not the first available account.
    model.accounts.insert(0, physical);
    report.validate().unwrap();
    report
}

fn draw_original(
    report: &api::AnalysisReport,
    inspector: &mut Inspector,
    account: OriginalAccount<'_>,
) -> String {
    let mut action = None;
    let text = rendered_at(
        &context(),
        &mut 0.0,
        vec![],
        egui::vec2(1200.0, 1800.0),
        |ui| {
            action = inspector.inspect(ui, InspectionTarget::Original { report, account });
        },
    );
    assert_eq!(
        action, None,
        "Original observations cannot navigate or choose a state"
    );
    text.into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn original_targets_resolve_exact_rows_without_a_procedure_or_native_graph() {
    let report = original_report();
    let model = report.minimum.as_deref().unwrap();
    let root = model
        .accounts
        .iter()
        .position(|row| row.root_template)
        .unwrap();
    let mut inspector = Inspector {
        tab: Tab::Storage,
        scope: super::super::super::minimum::Scope::Outcome,
        last_state: Some(usize::MAX),
        last_minimum: Some(minimum::Selection {
            summary: usize::MAX,
            procedure: usize::MAX,
            ..Default::default()
        }),
        ..Default::default()
    };
    for (target, balance, slot, excluded) in [
        (OriginalAccount::Root, "0xa1", "0xc3", "0xb2"),
        (OriginalAccount::Minimum(root), "0xa1", "0xc3", "0xb2"),
        (OriginalAccount::Minimum(0), "0xb2", "0xd4", "0xa1"),
        (OriginalAccount::Address(PHYSICAL), "0xb2", "0xd4", "0xa1"),
    ] {
        let text = draw_original(&report, &mut inspector, target);
        for expected in [
            "Original observations",
            "Balance",
            balance,
            "Original value",
            SLOT,
            slot,
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        for forbidden in [
            excluded,
            "Joined procedure input bounds",
            "Joined ADDRESS",
            "Joint outcome",
            "Native before",
        ] {
            assert!(!text.contains(forbidden), "unexpected {forbidden}: {text}");
        }
        assert_eq!(inspector.last_state, Some(usize::MAX));
        assert_eq!(inspector.last_minimum.unwrap().procedure, usize::MAX);
        assert_eq!(
            inspector.scope,
            super::super::super::minimum::Scope::Outcome
        );
    }
    inspector.tab = Tab::Frame;
    let text = draw_original(&report, &mut inspector, OriginalAccount::Root);
    assert!(text.contains("Logical ADDRESS") && text.contains("Root input template"));
    assert!(text.contains("original symbolic qualification"));
    assert!(!text.contains(PHYSICAL));
}

#[test]
fn invalid_original_id_address_and_template_as_physical_are_rejected_before_facts() {
    let report = original_report();
    let template = report
        .minimum
        .as_deref()
        .unwrap()
        .accounts
        .iter()
        .find(|row| row.root_template)
        .unwrap()
        .address
        .as_str();
    for tab in [Tab::Frame, Tab::Environment, Tab::Storage, Tab::Calldata] {
        let mut inspector = Inspector {
            tab,
            ..Default::default()
        };
        for target in [
            OriginalAccount::Minimum(usize::MAX),
            OriginalAccount::Address("0x0000000000000000000000000000000000000001"),
            OriginalAccount::Address("0x1111"),
            OriginalAccount::Address("0xgg11111111111111111111111111111111111111"),
            OriginalAccount::Address(template),
        ] {
            let text = draw_original(&report, &mut inspector, target);
            assert!(text.contains("original account is unavailable"), "{text}");
            for forbidden in [
                "Balance",
                "Logical ADDRESS",
                "Length",
                "Original value",
                SLOT,
                "0xa1",
                "0xb2",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "facts leaked for {target:?}: {text}"
                );
            }
        }
    }
}

#[test]
fn original_target_cannot_supply_instruction_or_joint_outcome_facts() {
    let report = original_report();
    for tab in [
        Tab::Stack,
        Tab::Memory,
        Tab::SsaValue,
        Tab::Outcomes,
        Tab::Returndata,
    ] {
        let text = draw_original(
            &report,
            &mut Inspector {
                tab,
                ..Default::default()
            },
            OriginalAccount::Root,
        );
        assert!(
            text.contains("This field has no original observation"),
            "{text}"
        );
        assert!(!text.contains("Paired path conditions") && !text.contains("Joined ADDRESS"));
    }
    let mut inspector = Inspector {
        tab: Tab::Storage,
        ..Default::default()
    };
    let mut action = None;
    let text = rendered_at(
        &context(),
        &mut 0.0,
        vec![],
        egui::vec2(1200.0, 1800.0),
        |ui| {
            action = inspector.inspect(
                ui,
                InspectionTarget::Source {
                    report: &report,
                    source: InspectionSource::Program(report.programs[0].id),
                },
            );
        },
    );
    assert_eq!(action, None);
    assert!(text.iter().any(|(label, _)| label == "Code hash"));
    assert!(
        !text
            .iter()
            .any(|(label, _)| label == "Balance" || label == SLOT)
    );
}

#[test]
fn captured_originals_without_minimum_do_not_read_transaction_stores() {
    let mut report = report().clone();
    report.minimum = None;
    let graph = Arc::make_mut(report.global.as_mut().unwrap());
    let row = graph
        .accounts
        .iter_mut()
        .find(|row| {
            Some(row.address.as_str())
                == match &graph.metadata.root_binding {
                    api::RootInputBinding::Namespace(root) => Some(root.coordinate.as_str()),
                    api::RootInputBinding::Physical(root) => Some(root.address.as_str()),
                }
        })
        .unwrap();
    row.balance = word("0xe5");
    row.storage = vec![api::StorageEntry {
        slot: SLOT.into(),
        value: word("0xf6"),
    }];
    for store in &mut graph.stores {
        for row in &mut store.accounts {
            row.balance = word("0xdead");
        }
    }
    report.validate().unwrap();
    let mut inspector = Inspector {
        tab: Tab::Storage,
        last_state: Some(usize::MAX),
        ..Default::default()
    };
    let text = draw_original(&report, &mut inspector, OriginalAccount::Root);
    assert!(
        text.contains("0xe5") && text.contains("0xf6") && text.contains(SLOT),
        "{text}"
    );
    assert!(!text.contains("0xdead"));
    assert!(text.contains("Root input template"), "{text}");
    assert_eq!(inspector.last_state, Some(usize::MAX));
    let text = draw_original(&report, &mut inspector, OriginalAccount::Minimum(0));
    assert!(text.contains("original account is unavailable") && !text.contains("0xe5"));
    inspector.tab = Tab::Calldata;
    let text = draw_original(&report, &mut inspector, OriginalAccount::Root);
    assert!(
        text.contains("Length") && text.contains("Base byte default"),
        "{text}"
    );
    inspector.tab = Tab::Frame;
    let text = draw_original(&report, &mut inspector, OriginalAccount::Root);
    assert!(text.contains("symbolic To"), "{text}");
    // The conditioned Native root uses these bits as an actual destination,
    // but the original input binding still identifies a namespace template.
    let namespace = &report.global.as_deref().unwrap().metadata.entry_address;
    let text = draw_original(&report, &mut inspector, OriginalAccount::Address(namespace));
    assert!(text.contains("original account is unavailable") && !text.contains("0xe5"));
}
