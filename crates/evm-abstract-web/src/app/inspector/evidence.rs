//! Acquisition and model-coverage evidence remain visible independently of selection.
use super::{errors, value};
use crate::{app::Selection, notation, palette};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{self as api, AnalysisReport};

pub(super) fn acquisition(ui: &mut Ui, report: &AnalysisReport) {
    let Some(acquisition) = &report.acquisition else {
        ui.label("Standalone bytecode: no RPC acquisition was performed.");
        return;
    };
    ui.label(format!(
        "{} rounds · {} RPC requests · {} observed accounts",
        acquisition.rounds,
        acquisition.requests,
        report.accounts.len()
    ));
    ui.label(format!(
        "Refinement acquired {} additional accounts and {} initial slots",
        acquisition.fetched_accounts.len(),
        acquisition.fetched_storage.len()
    ));
    ui.label(
        RichText::new("All observations use the pinned canonical block shown in Environment.")
            .small()
            .color(palette::MUTED),
    );
    ui.collapsing(
        format!(
            "Discovered accounts ({})",
            acquisition.fetched_accounts.len()
        ),
        |ui| {
            for address in &acquisition.fetched_accounts {
                ui.monospace(address);
            }
        },
    );
    ui.collapsing(
        format!("Refinement slots ({})", acquisition.fetched_storage.len()),
        |ui| {
            for slot in &acquisition.fetched_storage {
                ui.monospace(format!("{} · {}", slot.address, slot.slot));
            }
        },
    );
    ui.collapsing(
        format!("Failed accounts ({})", acquisition.failed_accounts.len()),
        |ui| {
            for address in &acquisition.failed_accounts {
                ui.monospace(address);
            }
        },
    );
    ui.collapsing(
        format!("Failed slots ({})", acquisition.failed_storage.len()),
        |ui| {
            for slot in &acquisition.failed_storage {
                ui.monospace(format!("{} · {}", slot.address, slot.slot));
            }
        },
    );
    for (index, failure) in acquisition.failures.iter().enumerate() {
        egui::CollapsingHeader::new(format!("{:?} · {}", failure.kind, failure.method))
            .id_salt(("rpc_failure", index))
            .default_open(true)
            .show(ui, |ui| errors::rpc(ui, failure));
    }
    if acquisition.failures.is_empty() {
        ui.label("No acquisition failures recorded.");
    }
}

pub(super) fn diagnostics(ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
    if report.diagnostics.is_empty() && report.frontiers.is_empty() {
        ui.label("No diagnostics or unexpanded frontiers reported.");
    } else {
        ui.label(RichText::new(diagnostic_counts(report)).small().color(palette::MUTED))
            .on_hover_text("Each diagnostic retains its abstract state. Repeated PCs can describe different states; these counts are not distinct instruction counts or failed transaction counts.");
    }
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        let presentation = diagnostic_presentation(diagnostic);
        // State identity and receipt index keep repeated PCs independently clickable.
        ui.push_id(("diagnostic", index, diagnostic.state), |ui| {
            if ui
                .selectable_label(false, notation::widget(ui, &presentation.label))
                .clicked()
            {
                *selection = Selection {
                    value: None,
                    state: Some(diagnostic.state),
                    pc: presentation.instruction_pc,
                };
            }
            if let Some(reason) = &presentation.reason {
                ui.label(RichText::new(reason).small().color(palette::MUTED));
            }
            if let Some(reason) = &diagnostic.precision {
                ui.label(
                    RichText::new(
                        "Value precision remains unknown; conservative propagation continues.",
                    )
                    .color(palette::WARNING),
                );
                errors::fields(ui, |ui| query(ui, reason));
            }
            if let Some(reduction) = &diagnostic.reduction {
                ui.label(
                    RichText::new(format!("Reduction: {reduction:?}"))
                        .small()
                        .color(palette::MUTED),
                );
            }
        });
    }
    for frontier in &report.frontiers {
        let location = match (frontier.from, frontier.pc) {
            (Some(state), Some(pc)) => format!("{} · 0x{pc:x}", Symbol::State(state)),
            (Some(state), None) => Symbol::State(state).to_string(),
            (None, Some(pc)) => format!("0x{pc:x}"),
            (None, None) => "Pending entry".into(),
        };
        if ui
            .selectable_label(
                false,
                notation::widget(
                    ui,
                    RichText::new(format!("{location} · {:?}", frontier.kind))
                        .color(palette::WARNING),
                ),
            )
            .clicked()
        {
            *selection = Selection {
                value: None,
                state: frontier.from,
                pc: frontier.pc,
            };
        }
        if let Some(range) = &frontier.range {
            let (evidence, limit_label, help) = match range.request.kind {
                api::RangeKind::MemoryPolicy => (
                    "Host memory materialization policy leaves these inputs unexpanded; EVM failure is not established.",
                    "Materialization policy limit",
                    "The historical max_memory_bytes policy bounds host materialization. Exceeding it does not exclude EVM success or establish EVM failure.",
                ),
                api::RangeKind::LogicalMemory => (
                    "Logical memory representability leaves these inputs unexpanded.",
                    "Word-rounded U256 end limit",
                    "For a nonempty access, offset + size and its rounding up to 32 bytes must fit in U256; the end limit is U256::MAX & !31. Empty accesses do not expand memory. This bound is independent of host materialization policy.",
                ),
                api::RangeKind::ReturnData => (
                    "Returndata range semantics remain pending for these inputs.",
                    "Returndata length",
                    "The bound is the complete returned sequence length.",
                ),
            };
            ui.label(RichText::new(evidence).color(palette::WARNING))
                .on_hover_text(help);
            errors::fields(ui, |ui| {
                value::named(ui, "Requested offset", &range.request.offset);
                value::named(ui, "Requested size", &range.request.size);
                value::named(ui, limit_label, &range.request.limit);
                value::named(ui, "In-range condition", &range.condition);
                value::text(
                    ui,
                    "Selected condition",
                    match range.nonzero {
                        Some(true) => "Nonzero",
                        Some(false) => "Zero",
                        None => "Both sides retained",
                    },
                );
            });
            super::symbolic::pending(ui, report, range.payload);
        }
        if let Some(payload) = frontier.payload {
            super::symbolic::pending(ui, report, payload);
        }
        match &frontier.reason {
            api::FrontierDetails::RpcAcquisition(failure) => errors::rpc(ui, failure),
            reason => errors::fields(ui, |ui| frontier_reason(ui, reason)),
        }
    }
}

fn diagnostic_counts(report: &AnalysisReport) -> String {
    format!(
        "{} abstract-state diagnostic entries · {} unexpanded frontiers",
        report.diagnostics.len(),
        report.frontiers.len()
    )
}

struct DiagnosticPresentation {
    label: String,
    reason: Option<String>,
    instruction_pc: Option<usize>,
}

fn diagnostic_presentation(diagnostic: &api::Diagnostic) -> DiagnosticPresentation {
    let mut presentation = DiagnosticPresentation {
        label: format!(
            "{} · PC 0x{:04x} · {:?}",
            Symbol::State(diagnostic.state),
            diagnostic.pc,
            diagnostic.kind
        ),
        reason: None,
        instruction_pc: Some(diagnostic.pc),
    };
    match &diagnostic.invalid_opcode {
        Some(api::InvalidOpcodeDiagnostic::Instruction(instruction)) => {
            presentation
                .label
                .push_str(&format!(" · 0x{:02x}", instruction.opcode));
            if let Some(mnemonic) = &instruction.mnemonic {
                presentation.label.push_str(&format!(" ({mnemonic})"));
            }
            let reason = match instruction.reason {
                api::InvalidOpcodeReason::ExplicitInvalid => {
                    "Explicit exceptional halt instruction".into()
                }
                api::InvalidOpcodeReason::UndefinedOpcode => {
                    "Undefined ordinary bytecode opcode".into()
                }
                api::InvalidOpcodeReason::UnavailableInFork(fork) => {
                    format!("Instruction is not enabled in fork {fork:?}")
                }
            };
            presentation.reason = Some(format!(
                "{reason}; this abstract path attempted this instruction."
            ));
        }
        Some(api::InvalidOpcodeDiagnostic::DelegationEntry(entry)) => {
            presentation.label = format!(
                "{} · Delegation entry (synthetic PC {}) · {:?}",
                Symbol::State(diagnostic.state),
                diagnostic.pc,
                diagnostic.kind
            );
            if let Some(opcode) = entry.opcode {
                presentation
                    .label
                    .push_str(&format!(" · captured first byte 0x{opcode:02x}"));
            }
            let mut reason = format!("{}; no ordinary instruction was decoded.", entry.detail);
            if let Some(unavailable) = &entry.byte_unavailable {
                reason.push_str(&format!(" Opcode byte unavailable: {unavailable}."));
            }
            presentation.reason = Some(reason);
            // Synthetic entry PC is not a source-code jump target.
            presentation.instruction_pc = None;
        }
        None => {}
    }
    presentation
}

/// The canvas and assistive live region share the typed diagnostic evidence.
pub(super) fn diagnostic_summary(report: &AnalysisReport) -> String {
    let mut summary = format!(
        "{}; counts are abstract-state entries, not instructions or transactions",
        diagnostic_counts(report)
    );
    for diagnostic in &report.diagnostics {
        let presentation = diagnostic_presentation(diagnostic);
        summary.push_str(&format!("; {}", presentation.label));
        if let Some(reason) = presentation.reason {
            summary.push_str(&format!("; {reason}"));
        }
    }
    summary
}
fn frontier_reason(ui: &mut Ui, reason: &api::FrontierDetails) {
    use api::FrontierDetails as R;
    match reason {
        R::Budget(resource) => value::text(ui, "Reached budget", format!("{resource:?}")),
        R::Work => value::text(ui, "Boundary", "Execution work budget"),
        R::SummaryWork => value::text(ui, "Boundary", "Summary work budget"),
        R::RecursiveNoTerminal => value::text(
            ui,
            "Boundary",
            "Recursive execution has no covered terminal behavior",
        ),
        R::CallDepth => value::text(ui, "Boundary", "External call depth"),
        R::Memory => value::text(ui, "Boundary", "Logical memory policy; unexpanded inputs"),
        R::ByteRange(reason) => match reason {
            api::ByteRangeBoundary::UnknownSize => value::text(
                ui,
                "Range representation",
                "No finite size or trusted symbolic term",
            ),
            api::ByteRangeBoundary::OffsetTooLarge(offset) => {
                value::text(ui, "Offset exceeds host index", offset)
            }
            api::ByteRangeBoundary::Overflow(range) => value::text(
                ui,
                "Host range overflow",
                format!("offset {} + size {}", range.offset, range.size),
            ),
            api::ByteRangeBoundary::TooLarge(range) => value::text(
                ui,
                "Concrete range policy",
                format!("{} exceeds {} bytes", range.requested, range.limit),
            ),
            api::ByteRangeBoundary::InputAlreadyBound => value::text(
                ui,
                "Source binding",
                "Original input identity already established",
            ),
        },
        R::StateJoin(reason) => value::text(
            ui,
            "State join boundary",
            match reason {
                api::JoinBoundary::Shape => "Frame or continuation shapes disagree".into(),
                api::JoinBoundary::IdentityExhausted => {
                    "No branch-selector identity available".into()
                }
                api::JoinBoundary::Alternatives(limit) => {
                    format!("Complete alternative limit {limit}")
                }
            },
        ),
        R::UnknownTarget => value::text(
            ui,
            "Boundary",
            "Call destination is not covered by captured code",
        ),
        R::MissingCode(address) => value::text(ui, "Missing code account", address),
        R::MissingStorage(slot) => {
            value::text(ui, "Storage owner", &slot.address);
            value::text(ui, "Unobserved slot", &slot.slot);
        }
        R::Precompile(address) => value::text(ui, "Unsupported direct precompile", address),
        R::PrecompileInput(address) => value::text(ui, "Unmodeled precompile input", address),
        R::UnsupportedOpcode(opcode) => {
            value::text(ui, "Unsupported opcode", format!("0x{opcode:02x}"))
        }
        R::Creation(reason) => creation(ui, reason),
        R::Relations(reason) => query(ui, reason),
        R::RpcAcquisition(_) => {} // Rendered in its own contextual grid above.
    }
}
fn creation(ui: &mut Ui, reason: &api::CreationReason) {
    use api::CreationReason as R;
    match reason {
        R::UnknownCreator => value::text(
            ui,
            "Creation boundary",
            "Creator logical address is symbolic",
        ),
        R::UnknownNonce(address) => value::text(ui, "Unknown creator nonce", address),
        R::UnknownCollision(address) => value::text(ui, "Unknown destination collision", address),
        R::UnknownEndowment => value::text(
            ui,
            "Creation boundary",
            "Endowment or balance transfer is unknown",
        ),
        R::UnknownInitCode => {
            value::text(ui, "Creation boundary", "Initcode bytes are not concrete")
        }
        R::UnknownRuntimeCode => value::text(
            ui,
            "Creation boundary",
            "Returned runtime bytes are not concrete",
        ),
        R::UnknownSalt => value::text(ui, "Creation boundary", "CREATE2 salt is not finite"),
        R::NonceOverflow(address) => value::text(ui, "Nonce exceeds 64 bits", address),
        R::ReservedAddress(address) => value::text(ui, "Reserved creation address", address),
    }
}
fn query(ui: &mut Ui, reason: &api::QueryBoundary) {
    use api::QueryBoundary as R;
    let label = match reason {
        R::Cancelled => "Relational query cancelled",
        R::Disabled => "Relational queries disabled",
        R::Configuration => "Invalid relational policy",
        R::ConstraintLimit => "Retained constraint bound",
        R::ScalarFactLimit => "Scalar fact bound",
        R::ScalarFactError(_) => "Scalar fact interpretation failed",
        R::ExpressionLimit => "Expression size / local work bound",
    };
    value::text(ui, "Query boundary", label);
    if let R::ScalarFactError(error) = reason {
        fact(ui, error);
    }
}

fn fact(ui: &mut Ui, error: &api::FactFailure) {
    use api::FactFailure as F;
    match error {
        F::InvalidBitIndex(index) => {
            value::text(ui, "Fact failure", "Bit index outside a 256-bit word");
            value::text(ui, "Bit index", index.to_string());
        }
        F::ConflictingBits => {
            value::text(ui, "Fact failure", "A bit was declared both zero and one")
        }
        F::InvalidBounds => value::text(
            ui,
            "Fact failure",
            "Lower interval bound exceeds upper bound",
        ),
        F::EmptyFiniteSet => value::text(ui, "Fact failure", "Finite membership set was empty"),
        F::InvalidModulus => value::text(ui, "Fact failure", "Congruence modulus was zero"),
        F::UnsupportedOperation(opcode) => {
            value::text(ui, "Fact failure", "Unsupported pure word operation");
            value::text(ui, "Opcode", format!("0x{opcode:02x}"));
        }
        F::InvalidOperation(detail) => {
            value::text(ui, "Fact failure", "Pure-operation operand count mismatch");
            value::text(ui, "Opcode", format!("0x{:02x}", detail.opcode));
            value::text(ui, "Expected operands", detail.expected.to_string());
            value::text(ui, "Actual operands", detail.actual.to_string());
        }
        F::Contradiction(symbol) => {
            value::text(ui, "Fact failure", "Numeric facts contradict");
            value::text(ui, "Local fact symbol", symbol.to_string());
        }
        F::OriginContradiction(symbol) => {
            value::text(ui, "Fact failure", "Origin declarations contradict");
            value::text(ui, "Local fact symbol", symbol.to_string());
        }
        F::Capacity(detail) => {
            value::text(ui, "Fact failure", "Semantic atom capacity exceeded");
            value::text(ui, "Maximum atoms", detail.max_atoms.to_string());
            value::text(ui, "Required atoms", detail.required.to_string());
        }
    }
}
