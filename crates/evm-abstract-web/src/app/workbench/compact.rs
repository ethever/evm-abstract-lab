//! One mobile navigation row keeps the source and Back visible. Secondary
//! navigation, evidence and preferences remain explicit in More.

use crate::app::{
    View, Workspace, inspector,
    model::{navigation::Intent, presentation::ResolvedContext},
};
use crate::{
    controls::{self, Density},
    palette,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisNamespace, HybridNativeBinding, ProcessOwner};

impl Workspace {
    pub(super) fn compact_navigation_bar(&mut self, ui: &mut Ui) {
        let mut intent = None;
        let mut namespace = None;
        let mut inspect = None;
        Density::Touch.scope(ui, |ui| {
            ui.horizontal(|ui| {
                if controls::action_enabled(
                    ui,
                    self.report_view.navigator().can_back(),
                    "Back",
                    Density::Touch,
                )
                .clicked()
                {
                    intent = Some(Intent::Back);
                }
                let details = matches!(self.view, View::Ssa | View::Split)
                    || (self.view == View::Graph
                        && self.report_view.namespace() == AnalysisNamespace::Native
                        && self.graph.content_label() == "SSA");
                let details_width = if details {
                    ui.painter()
                        .layout_no_wrap(
                            "SSA details".into(),
                            egui::FontId::proportional(13.0),
                            palette::TEXT,
                        )
                        .size()
                        .x
                        + 12.0
                        + ui.spacing().item_spacing.x
                } else {
                    0.0
                };
                let width =
                    (ui.available_width() - 56.0 - details_width - ui.spacing().item_spacing.x)
                        .max(0.0);
                let source = self.report.as_ref().and_then(|report| {
                    let focus = &self.report_view.navigator().current()?.focus;
                    focus
                        .source
                        .as_ref()?
                        .program()
                        .and_then(|id| self.report_view.presentation(report).program(id))
                });
                let mut label = format!(
                    "{} · {}",
                    if self.report_view.namespace() == AnalysisNamespace::Native {
                        "Native"
                    } else {
                        "MAY"
                    },
                    source.map_or_else(
                        || "Uncaptured source".into(),
                        |program| address(program.code_address.as_deref().unwrap_or("Input code"))
                    )
                );
                let mut description = source.map_or_else(
                    || "Source bytes were not captured".into(),
                    |program| {
                        format!(
                            "Code: {}\n{}",
                            program.code_address.as_deref().unwrap_or("Input code"),
                            program.code_hash
                        )
                    },
                );
                if let Some((report, location)) = self
                    .report
                    .as_ref()
                    .zip(self.report_view.navigator().current())
                {
                    let context = self
                        .report_view
                        .presentation(report)
                        .resolve_context(&location.focus);
                    let (owner, is_static) = match context {
                        Ok(Some(ResolvedContext::Graph { state, frame, .. })) => (
                            state.storage_address.clone(),
                            state
                                .process
                                .as_ref()
                                .map(|process| process.is_static)
                                .or_else(|| frame.map(|frame| frame.is_static)),
                        ),
                        Ok(Some(ResolvedContext::Minimum { procedure, .. })) => {
                            procedure.process.as_ref().map_or((None, None), |process| {
                                let owner = match &process.owner {
                                    ProcessOwner::Named(value) => value.clone(),
                                    ProcessOwner::RootNamespace => "root".into(),
                                    ProcessOwner::Residual => "residual".into(),
                                };
                                (Some(owner), Some(process.is_static))
                            })
                        }
                        _ => (None, None),
                    };
                    if let Some(owner) = owner
                        && !source.is_some_and(|program| {
                            program.code_address.as_deref() == Some(owner.as_str())
                        })
                    {
                        label.push_str(&format!(" · owner {}", address(&owner)));
                        description.push_str(&format!("\nStorage owner: {owner}"));
                    }
                    if is_static == Some(true) {
                        label.push_str(" · static");
                        description.push_str("\nRead-only context");
                    }
                }
                ui.add_sized(
                    [width, Density::Touch.height()],
                    egui::Label::new(RichText::new(label).color(palette::TEXT)).truncate(),
                )
                .on_hover_text(description);
                if details {
                    self.ssa_details_control(ui, Density::Touch);
                }
                controls::menu_button(ui, "More", Density::Touch, |ui| {
                    if controls::action_enabled(
                        ui,
                        self.report_view.navigator().can_forward(),
                        "Forward",
                        Density::Touch,
                    )
                    .clicked()
                    {
                        intent = Some(Intent::Forward);
                        ui.close();
                    }
                    if controls::action(ui, "Entry", Density::Touch).clicked()
                        && let Some(report) = &self.report
                        && let Ok(focus) = self
                            .report_view
                            .presentation(report)
                            .entry_focus(self.report_view.identity())
                    {
                        intent = Some(Intent::Open(focus));
                        ui.close();
                    }
                    ui.separator();
                    let global = self.report.as_ref().is_some_and(|report| {
                        report.minimum.is_some()
                            || report.graph(AnalysisNamespace::Baseline).is_some()
                    });
                    if controls::action_enabled(ui, global, "Global model", Density::Touch)
                        .clicked()
                    {
                        namespace = Some(AnalysisNamespace::Baseline);
                        ui.close();
                    }
                    let native = self
                        .report
                        .as_ref()
                        .is_some_and(|report| report.native.is_some());
                    if controls::action_enabled(ui, native, "Detailed paths", Density::Touch)
                        .clicked()
                    {
                        namespace = Some(AnalysisNamespace::Native);
                        ui.close();
                    }
                    ui.separator();
                    if !details {
                        self.ssa_details_control(ui, Density::Touch);
                    }
                    for (tab, label) in [
                        (inspector::Tab::Timing, "Timing"),
                        (inspector::Tab::Coverage, "Coverage"),
                        (inspector::Tab::Diagnostics, "Diagnostics"),
                        (inspector::Tab::Outcomes, "Terminal coverage"),
                    ] {
                        if controls::action(ui, label, Density::Touch).clicked() {
                            inspect = Some(tab);
                            ui.close();
                        }
                    }
                });
            });
        });
        if let Some(report) = &self.report
            && self.report_view.namespace() == AnalysisNamespace::Native
            && let Some(hybrid) = &report.hybrid
            && let HybridNativeBinding::SymbolicOwnerNamespace(binding) =
                &hybrid.native_coverage().detail_binding
        {
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "Condition: Root ADDRESS = {}",
                        super::super::directory::short(&binding.namespace)
                    ))
                    .small()
                    .color(palette::WARNING),
                )
                .truncate(),
            )
            .on_hover_text(format!("Root ADDRESS = {}", binding.namespace));
        }
        if self.report_view.namespace() == AnalysisNamespace::Native
            && self.report.as_ref().is_some_and(|report| {
                report
                    .hybrid
                    .as_ref()
                    .and_then(|hybrid| hybrid.rpc.as_ref())
                    .is_some_and(|rpc| !rpc.detail_at_current_revision)
            })
        {
            ui.colored_label(palette::WARNING, "Earlier input conditions");
        }
        if let Some(intent) = intent {
            self.navigate(intent, None);
            ui.ctx().request_discard("mobile navigation");
        }
        if let Some(namespace) = namespace {
            self.choose_context(namespace);
        }
        if let Some(tab) = inspect {
            self.directory.open = false;
            self.inspector.expanded = true;
            self.inspector.tab = tab;
        }
    }

    pub(super) fn ssa_details_control(&mut self, ui: &mut Ui, density: Density) {
        let mut full = self.ssa_display == crate::ssa_display::SsaDisplay::Full;
        if controls::toggle(ui, &mut full, "SSA details", density)
            .on_hover_text("Show full instruction, stack and effect-chain detail in SSA views")
            .changed()
        {
            self.ssa_display = if full {
                crate::ssa_display::SsaDisplay::Full
            } else {
                crate::ssa_display::SsaDisplay::Compact
            };
        }
    }
}

fn address(value: &str) -> String {
    if value.starts_with("0x") && value.len() > 14 && value.is_ascii() {
        format!("…{}", &value[value.len() - 6..])
    } else {
        value.into()
    }
}
