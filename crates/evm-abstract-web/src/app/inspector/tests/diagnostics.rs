//! Typed opcode evidence, state identity and synthetic entry navigation.
use super::*;

fn instruction(
    state: usize,
    pc: usize,
    opcode: u8,
    mnemonic: Option<&str>,
    reason: api::InvalidOpcodeReason,
) -> api::Diagnostic {
    api::Diagnostic {
        precision: None,
        state,
        pc,
        kind: api::DiagnosticKind::InvalidOpcode,
        reduction: None,
        invalid_opcode: Some(api::InvalidOpcodeDiagnostic::Instruction(
            api::InvalidInstruction {
                opcode,
                mnemonic: mnemonic.map(str::to_owned),
                reason,
            },
        )),
        detail: "This debug string is not the authoritative opcode evidence".into(),
    }
}

#[test]
fn invalid_opcode_canvas_and_accessibility_distinguish_all_three_causes() {
    let mut report = crate::tests::report();
    // The root source deliberately disagrees with every diagnostic. Presentation
    // must consume the receipt's opcode rather than indexing root bytecode.
    report.programs[0].bytecode = "0x00".into();
    report.frontiers.clear();
    report.diagnostics = vec![
        instruction(
            123,
            0,
            0xfe,
            Some("INVALID"),
            api::InvalidOpcodeReason::ExplicitInvalid,
        ),
        instruction(
            456,
            0,
            0xef,
            None,
            api::InvalidOpcodeReason::UndefinedOpcode,
        ),
        instruction(
            789,
            1,
            0x1e,
            Some("CLZ"),
            api::InvalidOpcodeReason::UnavailableInFork(api::Fork::Cancun),
        ),
    ];
    let context = egui::Context::default();
    crate::notation::initialize_fonts(&context);
    let mut time = 0.0;
    let mut selection = Selection::default();
    let text = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1200.0, 600.0),
        |ui| evidence::diagnostics(ui, &report, &mut selection),
    );
    let labels: Vec<_> = text.iter().map(|(text, _)| text.as_str()).collect();
    for expected in [
        "3 abstract-state diagnostic entries · 0 unexpanded frontiers",
        "σ123 · PC 0x0000 · InvalidOpcode · 0xfe (INVALID)",
        "σ456 · PC 0x0000 · InvalidOpcode · 0xef",
        "σ789 · PC 0x0001 · InvalidOpcode · 0x1e (CLZ)",
        "Explicit exceptional halt instruction; this abstract path attempted this instruction.",
        "Undefined ordinary bytecode opcode; this abstract path attempted this instruction.",
        "Instruction is not enabled in fork Cancun; this abstract path attempted this instruction.",
    ] {
        assert!(labels.contains(&expected), "missing {expected}: {labels:?}");
    }
    assert!(!labels.iter().any(|text| text.contains("0xef (INVALID)")));
    assert!(!labels.iter().any(|text| text.contains("0x00 (INVALID)")));
    let inspector = Inspector {
        tab: Tab::Diagnostics,
        ..Inspector::default()
    };
    let summary = evm_abstract_notation::normalize_subscripts(
        &inspector.accessible_summary(&report, selection),
    );
    for expected in [
        "0xfe (INVALID)",
        "0xef",
        "0x1e (CLZ)",
        "fork Cancun",
        "counts are abstract-state entries, not instructions or transactions",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[test]
fn repeated_invalid_pc_keeps_each_abstract_state_and_exact_jump() {
    let mut report = crate::tests::report();
    report.frontiers.clear();
    report.diagnostics = vec![
        instruction(
            123,
            17,
            0xfe,
            Some("INVALID"),
            api::InvalidOpcodeReason::ExplicitInvalid,
        ),
        instruction(
            456,
            17,
            0xfe,
            Some("INVALID"),
            api::InvalidOpcodeReason::ExplicitInvalid,
        ),
    ];
    let context = egui::Context::default();
    crate::notation::initialize_fonts(&context);
    let mut time = 0.0;
    let mut selection = Selection::default();
    let text = rendered(&context, &mut time, vec![], |ui| {
        evidence::diagnostics(ui, &report, &mut selection)
    });
    for diagnostic in &report.diagnostics {
        let position = text
            .iter()
            .find(|(text, _)| text.starts_with(&format!("σ{} · PC", diagnostic.state)))
            .unwrap()
            .1
            .center();
        click(&context, &mut time, position, |ui| {
            evidence::diagnostics(ui, &report, &mut selection)
        });
        assert_eq!(
            selection,
            Selection {
                state: Some(diagnostic.state),
                pc: Some(17),
                value: None
            }
        );
    }
    assert_eq!(report.diagnostics.len(), 2);
}

#[test]
fn delegation_entry_shows_byte_source_and_missing_reason_without_instruction_jump() {
    let mut report = crate::tests::report();
    report.frontiers.clear();
    report.diagnostics = vec![api::Diagnostic {
        precision: None,
        state: 42,
        pc: 0,
        kind: api::DiagnosticKind::InvalidOpcode,
        reduction: None,
        invalid_opcode: Some(api::InvalidOpcodeDiagnostic::DelegationEntry(
            api::InvalidDelegationEntry {
                opcode: Some(0xef),
                detail: "Delegation target is also delegated".into(),
                byte_unavailable: None,
            },
        )),
        detail: "Ignored debug string".into(),
    }];
    let context = egui::Context::default();
    crate::notation::initialize_fonts(&context);
    let mut time = 0.0;
    let mut selection = Selection::default();
    let text = rendered(&context, &mut time, vec![], |ui| {
        evidence::diagnostics(ui, &report, &mut selection)
    });
    let label = text
        .iter()
        .find(|(text, _)| text.contains("captured first byte 0xef"))
        .expect("captured entry byte must remain identifiable");
    assert!(label.0.contains("Delegation entry (synthetic PC 0)"));
    click(&context, &mut time, label.1.center(), |ui| {
        evidence::diagnostics(ui, &report, &mut selection)
    });
    assert_eq!(
        selection,
        Selection {
            state: Some(42),
            pc: None,
            value: None
        }
    );

    report.diagnostics[0].invalid_opcode = Some(api::InvalidOpcodeDiagnostic::DelegationEntry(
        api::InvalidDelegationEntry {
            opcode: None,
            detail: "Delegation cycle".into(),
            byte_unavailable: Some("Active account has no captured raw code".into()),
        },
    ));
    let text = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1200.0, 600.0),
        |ui| evidence::diagnostics(ui, &report, &mut selection),
    );
    assert!(text.iter().any(|(text, _)| text == "Delegation cycle; no ordinary instruction was decoded. Opcode byte unavailable: Active account has no captured raw code."));
    assert!(!text.iter().any(|(text, _)| text.contains("PC 0x0000")
        || text.contains("captured first byte")
        || text.contains("0xfe")));
}
