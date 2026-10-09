//! Acquisition and model-coverage evidence remain visible independently of selection.
use super::{errors, value};
use crate::{app::Selection, palette};
use egui::{RichText, Ui};
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
    }
    for diagnostic in &report.diagnostics {
        if ui
            .selectable_label(
                false,
                format!(
                    "S{} · 0x{:04x} · {:?}",
                    diagnostic.state, diagnostic.pc, diagnostic.kind
                ),
            )
            .clicked()
        {
            *selection = Selection {
                state: Some(diagnostic.state),
                pc: Some(diagnostic.pc),
            };
        }
        if let Some(reduction) = &diagnostic.reduction {
            ui.label(
                RichText::new(format!("Reduction: {reduction:?}"))
                    .small()
                    .color(palette::MUTED),
            );
        }
    }
    for frontier in &report.frontiers {
        let location = match (frontier.from, frontier.pc) {
            (Some(state), Some(pc)) => format!("S{state} · 0x{pc:x}"),
            (Some(state), None) => format!("S{state}"),
            (None, Some(pc)) => format!("0x{pc:x}"),
            (None, None) => "Pending entry".into(),
        };
        if ui
            .selectable_label(
                false,
                RichText::new(format!("{location} · {:?}", frontier.kind)).color(palette::WARNING),
            )
            .clicked()
        {
            *selection = Selection {
                state: frontier.from,
                pc: frontier.pc,
            };
        }
        match &frontier.reason {
            api::FrontierDetails::RpcAcquisition(failure) => errors::rpc(ui, failure),
            reason => errors::fields(ui, |ui| frontier_reason(ui, reason)),
        }
    }
}
fn frontier_reason(ui: &mut Ui, reason: &api::FrontierDetails) {
    use api::FrontierDetails as R;
    match reason {
        R::Budget(resource) => value::text(ui, "Reached budget", format!("{resource:?}")),
        R::Work => value::text(ui, "Boundary", "Execution work budget"),
        R::SummaryWork => value::text(ui, "Boundary", "Summary work budget"),
        R::CallDepth => value::text(ui, "Boundary", "External call depth"),
        R::Memory => value::text(ui, "Boundary", "Modeled memory bound"),
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
        R::ExpressionLimit => "Expression size / encoding work bound",
        R::Unsupported(_) => "Unsupported pure operation",
        R::ResourceLimit => "Native solver resource allowance",
        R::SolverUnknown(_) => "Solver could not decide",
        R::SolverError(_) => "Solver binding or encoding failed",
        R::ModelUnavailable => "Model value unavailable",
    };
    value::text(ui, "Query boundary", label);
    match reason {
        R::ScalarFactError(error) => fact(ui, error),
        R::SolverUnknown(detail) => {
            value::text(ui, "Provider", format!("{:?}", detail.provider));
            value::text(ui, "Native reason", &detail.reason);
        }
        R::SolverError(detail) => {
            value::text(ui, "Provider", format!("{:?}", detail.provider));
            value::text(ui, "Binding diagnostic", &detail.message);
        }
        R::Unsupported(opcode) => value::text(ui, "Opcode", format!("0x{opcode:02x}")),
        _ => {}
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
