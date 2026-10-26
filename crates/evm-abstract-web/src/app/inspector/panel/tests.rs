use egui::{Event, FullOutput, Id, Modifiers, PanelState, Pos2, RawInput, Rect, Vec2};

use crate::{
    Workspace,
    app::{Selection, View, inspector::Tab},
};

struct Workbench {
    ctx: egui::Context,
    workspace: Workspace,
    size: Vec2,
    time: f64,
}

impl Workbench {
    fn new(size: Vec2) -> Self {
        let ctx = egui::Context::default();
        crate::palette::configure(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        Self {
            ctx,
            workspace: crate::tests::ready(),
            size,
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        self.time += 0.02;
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..RawInput::default()
        };
        self.workspace.prepare_input(&mut input);
        let mut output = self.ctx.run_ui(input, |ui| {
            assert!(self.workspace.show(ui).is_none(), "UI must not reanalyse");
        });
        output.textures_delta.clear();
        output
    }

    fn rect(&self, name: &str) -> Rect {
        PanelState::load(&self.ctx, Id::new(name))
            .expect("panel rendered")
            .outer_rect
    }

    fn click(&mut self, label: &str) {
        let output = self.frame(vec![]);
        let position = label_rect(&output, label).center();
        for pressed in [true, false] {
            self.frame(vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
        self.frame(vec![]);
    }

    fn drag_to_height(&mut self, height: f32) {
        self.frame(vec![]);
        let rect = self.rect("world_inspector");
        let start = rect.center_top();
        let end = egui::pos2(start.x, rect.bottom() - height);
        self.frame(vec![Event::PointerMoved(start)]);
        self.frame(vec![Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(vec![Event::PointerMoved(end)]);
        self.frame(vec![]);
        self.frame(vec![Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(vec![]);
    }

    fn toggle_key(&mut self) {
        self.frame(vec![key(true), key(false)]);
    }
}

fn key(pressed: bool) -> Event {
    Event::Key {
        key: egui::Key::J,
        physical_key: Some(egui::Key::J),
        pressed,
        repeat: false,
        modifiers: Modifiers::CTRL,
    }
}

fn labels(output: &FullOutput) -> Vec<(String, Rect)> {
    fn collect(shape: &egui::Shape, labels: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => labels.push((
                evm_abstract_notation::normalize_subscripts(text.galley.text()),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, labels);
                }
            }
            _ => {}
        }
    }
    let mut result = vec![];
    for shape in &output.shapes {
        collect(&shape.shape, &mut result);
    }
    result
}

fn label_rect(output: &FullOutput, label: &str) -> Rect {
    labels(output)
        .into_iter()
        .find_map(|(text, rect)| (text == label).then_some(rect))
        .unwrap_or_else(|| panic!("label {label:?} must be painted"))
}

#[test]
fn desktop_sources_keep_their_height_beside_a_tall_docked_inspector() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.frame(vec![]);
    workbench.drag_to_height(2000.0);
    let preferred = workbench.rect("world_inspector").height();
    let selected = workbench.workspace.selection;
    let route = workbench.workspace.view;
    workbench.click("Sources");
    assert!(workbench.workspace.inspector.expanded);
    assert!(!workbench.workspace.inspector.maximized());
    let sources = workbench.rect("world_directory");
    let inspector = workbench.rect("world_inspector");
    assert!(
        sources.height() > 600.0,
        "the chooser must not inherit the graph's 72px remainder"
    );
    assert!(inspector.left() >= sources.right() - 1.0);
    let filter = workbench
        .ctx
        .read_response(Id::new("source_context_filter"))
        .unwrap();
    assert!(sources.contains_rect(filter.rect));
    assert!((inspector.height() - preferred).abs() < 2.0);
    workbench
        .workspace
        .choose_context(evm_abstract_protocol::AnalysisNamespace::Native);
    workbench.frame(vec![]);
    let filter = workbench
        .ctx
        .read_response(Id::new("source_context_filter"))
        .unwrap();
    assert!(workbench.rect("world_directory").contains_rect(filter.rect));
    workbench.click("Sources");
    assert!(!workbench.workspace.directory.open);
    assert!((workbench.rect("world_inspector").height() - preferred).abs() < 2.0);
    assert_eq!(workbench.workspace.selection, selected);
    assert_eq!(workbench.workspace.view, route);
}

#[test]
fn sources_and_context_chooser_remain_reachable_after_maximize() {
    for context_choice in [false, true] {
        let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
        workbench.frame(vec![]);
        workbench.drag_to_height(430.0);
        let preferred = workbench.rect("world_inspector").height();
        let focus = workbench
            .workspace
            .report_view
            .navigator()
            .current()
            .unwrap()
            .focus
            .clone();
        let route = workbench.workspace.view;
        workbench.click("Maximize");
        assert!(workbench.workspace.inspector.maximized());
        if context_choice {
            workbench
                .workspace
                .choose_context(evm_abstract_protocol::AnalysisNamespace::Native);
            workbench.frame(vec![]);
        } else {
            workbench.click("Sources");
        }
        assert!(workbench.workspace.directory.open);
        assert!(!workbench.workspace.inspector.expanded);
        let filter = workbench
            .ctx
            .read_response(Id::new("source_context_filter"))
            .expect("the requested chooser must render its real editor");
        assert!(filter.rect.is_positive());
        assert!(workbench.rect("world_directory").contains_rect(filter.rect));
        assert_eq!(workbench.workspace.view, route);
        assert_eq!(
            workbench
                .workspace
                .report_view
                .navigator()
                .current()
                .unwrap()
                .focus,
            focus
        );
        workbench.click("Inspect");
        assert!(
            workbench.workspace.inspector.maximized(),
            "retain the user's full-panel preference"
        );
        assert!(!workbench.workspace.sources_visible());
        assert!(
            workbench.workspace.directory.open,
            "retain the source-panel preference while Inspect is maximized"
        );
        workbench.click("Restore");
        assert!((workbench.rect("world_inspector").height() - preferred).abs() < 2.0);
        assert_eq!(workbench.workspace.view, route);
        assert_eq!(
            workbench
                .workspace
                .report_view
                .navigator()
                .current()
                .unwrap()
                .focus,
            focus
        );
    }
}

#[test]
fn dragging_inspect_can_use_most_of_the_result_workspace() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.frame(vec![]);
    assert_eq!(workbench.rect("world_inspector").height(), 210.0);
    workbench.drag_to_height(650.0);
    let rect = workbench.rect("world_inspector");
    assert!((rect.height() - 650.0).abs() < 2.0, "{rect:?}");
    assert!(rect.height() > workbench.size.y * 0.38);
    workbench.drag_to_height(2000.0);
    let context = workbench.rect("result_context");
    let inspector = workbench.rect("world_inspector");
    let status = workbench.rect("workspace_status");
    assert!(inspector.top() >= context.bottom());
    assert!(inspector.bottom() <= status.top());
    assert!((inspector.top() - context.bottom() - super::PANE_REMAINDER).abs() < 2.0);
}

#[test]
fn short_viewports_use_a_moderate_default_but_allow_a_larger_user_height() {
    let mut workbench = Workbench::new(egui::vec2(390.0, 300.0));
    workbench.frame(vec![]);
    let initial = workbench.rect("world_inspector").height();
    assert!(initial <= 300.0 * 0.38 + 1.0);
    workbench.drag_to_height(175.0);
    let available = workbench.rect("workspace_status").top()
        - workbench.rect("result_context").bottom()
        - super::PANE_REMAINDER;
    let chosen = workbench.rect("world_inspector").height();
    assert!(chosen > initial + 1.0);
    assert!((chosen - 175.0_f32.min(available)).abs() < 2.0);
}

#[test]
fn long_diagnostics_do_not_grow_the_chosen_docked_height() {
    let mut workbench = Workbench::new(egui::vec2(390.0, 300.0));
    workbench.workspace.inspector.tab = Tab::Diagnostics;
    workbench.frame(vec![]);
    let height = workbench.rect("world_inspector").height();
    std::sync::Arc::make_mut(
        workbench
            .workspace
            .report
            .as_mut()
            .unwrap()
            .native
            .as_mut()
            .unwrap(),
    )
    .diagnostics = (0..100)
        .map(|pc| evm_abstract_protocol::Diagnostic {
            precision: None,
            state: 0,
            pc,
            kind: evm_abstract_protocol::DiagnosticKind::OpaqueResult,
            reduction: None,
            invalid_opcode: None,
            detail: "A detailed diagnostic must remain independently scrollable. ".repeat(20),
        })
        .collect();
    workbench.frame(vec![]);
    assert_eq!(workbench.rect("world_inspector").height(), height);
    workbench.drag_to_height(150.0);
    let height = workbench.rect("world_inspector").height();
    workbench.frame(vec![]);
    assert_eq!(workbench.rect("world_inspector").height(), height);
}

#[test]
fn maximized_inspect_keeps_context_and_restores_user_height_and_selection() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.workspace.selection = Selection {
        state: Some(1),
        pc: Some(5),
        value: None,
    };
    workbench.workspace.inspector.tab = Tab::Environment;
    workbench.drag_to_height(530.0);
    let height = workbench.rect("world_inspector").height();
    let selection = workbench.workspace.selection;
    workbench.click("Maximize");
    assert!(workbench.workspace.inspector.maximized());
    assert!(
        workbench
            .workspace
            .accessible_status()
            .contains(" · Maximized")
    );
    let output = workbench.frame(vec![]);
    let restore = label_rect(&output, "Restore");
    let hide = label_rect(&output, "Hide");
    let context = workbench.rect("result_context");
    let status = workbench.rect("workspace_status");
    let header = workbench.rect("workspace_header");
    assert!(header.bottom() <= context.top());
    assert!(restore.top() >= context.bottom());
    assert!(restore.top() < context.bottom() + 12.0);
    assert!(status.top() > hide.bottom());
    assert!(Rect::from_min_size(Pos2::ZERO, workbench.size).contains_rect(hide));
    workbench.click("Restore");
    assert!(!workbench.workspace.inspector.maximized());
    assert!(
        !workbench
            .workspace
            .accessible_status()
            .contains(" · Maximized")
    );
    assert_eq!(workbench.rect("world_inspector").height(), height);
    assert_eq!(workbench.workspace.selection, selection);
    assert_eq!(workbench.workspace.inspector.tab, Tab::Environment);
}

#[test]
fn hide_and_reopen_preserve_maximized_mode_tab_and_inspector_scroll() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.workspace.inspector.tab = Tab::Environment;
    workbench.frame(vec![]);
    let scroll_id = Id::new("inspector_contents").with(egui::IdSalt::new("environment_details"));
    let mut scroll = egui::scroll_area::State::load(&workbench.ctx, scroll_id)
        .expect("environment inspector has a scroll area");
    scroll.offset.y = 42.0;
    scroll.store(&workbench.ctx, scroll_id);
    workbench.frame(vec![]);
    let offset = egui::scroll_area::State::load(&workbench.ctx, scroll_id)
        .unwrap()
        .offset;
    assert!(offset.y > 0.0);
    workbench.click("Hide");
    assert!(!workbench.workspace.inspector.expanded);
    workbench.toggle_key();
    assert!(workbench.workspace.inspector.expanded);
    assert_eq!(workbench.workspace.inspector.tab, Tab::Environment);
    assert_eq!(
        egui::scroll_area::State::load(&workbench.ctx, scroll_id)
            .unwrap()
            .offset,
        offset
    );
    workbench.click("Maximize");
    workbench.click("Hide");
    assert!(!workbench.workspace.inspector.expanded);
    assert!(workbench.workspace.inspector.panel.maximized());
    workbench.toggle_key();
    assert!(workbench.workspace.inspector.maximized());
    assert_eq!(workbench.workspace.inspector.tab, Tab::Environment);
    label_rect(&workbench.frame(vec![]), "Restore");
}

#[test]
fn short_and_narrow_viewports_clamp_temporarily_then_restore_the_preference() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.drag_to_height(650.0);
    let preferred = workbench.rect("world_inspector").height();
    for size in [egui::vec2(400.0, 1000.0), egui::vec2(400.0, 260.0)] {
        workbench.size = size;
        workbench.frame(vec![]);
        let output = workbench.frame(vec![]);
        let inspector = workbench.rect("world_inspector");
        let context = workbench.rect("result_context");
        let status = workbench.rect("workspace_status");
        assert!(
            inspector.top() >= context.bottom(),
            "{size:?}: {inspector:?}"
        );
        assert!(inspector.bottom() <= status.top());
        assert!(inspector.left() >= 0.0 && inspector.right() <= size.x);
        assert!(inspector.height() > 0.0 && inspector.height() <= preferred);
        assert!(inspector.contains_rect(label_rect(&output, "Hide")));
        assert!(inspector.contains_rect(label_rect(&output, "Maximize")));
        workbench.click("Maximize");
        let output = workbench.frame(vec![]);
        assert!(
            Rect::from_min_size(Pos2::ZERO, size).contains_rect(label_rect(&output, "Restore"))
        );
        workbench.click("Restore");
    }
    workbench.size = egui::vec2(1440.0, 1000.0);
    workbench.frame(vec![]);
    assert_eq!(workbench.rect("world_inspector").height(), preferred);
}

#[test]
fn maximizing_and_restoring_keep_the_existing_cfg_camera() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    workbench.workspace.view = View::Graph;
    workbench.frame(vec![]);
    workbench.frame(vec![]);
    workbench
        .workspace
        .graph
        .zoom_at(egui::vec2(140.0, 120.0), 0.7);
    let before = workbench.frame(vec![]);
    let camera = workbench.workspace.graph.accessible_summary();
    let report = workbench.workspace.report.as_ref().unwrap();
    let state = report
        .native
        .as_ref()
        .unwrap()
        .cfg
        .iter()
        .find(|state| state.id == 1)
        .unwrap();
    let source = report.source_block(state.source_block().unwrap()).unwrap();
    let instruction = &source.instructions[0];
    let title = format!("Block 0x{:04x}", source.start_pc);
    let source_row = format!("{:04x}  {}", instruction.pc, instruction.name);
    let (node_title, node) = labels(&before)
        .into_iter()
        .find(|(text, _)| {
            text == &title
                || text.starts_with(&source_row)
                || text.split("  ·").next() == Some("σ1")
        })
        .expect("the unselected node's actual source instruction is visible in the CFG");
    let selection = workbench.workspace.selection;
    workbench.click("Maximize");
    workbench.click("Restore");
    assert_eq!(workbench.workspace.graph.accessible_summary(), camera);
    assert_eq!(workbench.workspace.selection, selection);
    assert_eq!(label_rect(&workbench.frame(vec![]), &node_title), node);
}

#[test]
fn maximizing_and_restoring_keep_the_source_reading_position() {
    let mut workbench = Workbench::new(egui::vec2(1440.0, 1000.0));
    let mut report = crate::tests::report();
    let mut block = report.model.programs[0].blocks[0].clone();
    let prototype = block.instructions[0].clone();
    block.instructions = (0..800)
        .map(|pc| {
            let mut instruction = prototype.clone();
            instruction.pc = pc;
            instruction.name = format!("ROW_{pc:04}");
            instruction
        })
        .collect();
    report.model.programs[0] = crate::tests::source_program(
        0,
        report.model.programs[0].code_address.clone(),
        vec![block],
    );
    for state in &mut report.cfg {
        state.basic_block = Some(0);
        state.start_pc = Some(0);
    }
    for state in &mut report.states {
        state.entry.frames[0].basic_block = Some(0);
    }
    workbench
        .workspace
        .receive(Ok(evm_abstract_protocol::AnalyzeReply {
            result: Ok((report).into()),
        }));
    workbench.workspace.view = View::Disassembly;
    workbench.frame(vec![]);
    workbench.frame(vec![]);
    workbench.frame(vec![
        Event::PointerMoved(egui::pos2(400.0, 250.0)),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1200.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        },
    ]);
    for _ in 0..30 {
        workbench.frame(vec![]);
    }
    fn rows(output: &FullOutput) -> Vec<(String, Pos2)> {
        labels(output)
            .into_iter()
            .filter_map(|(text, rect)| {
                text.find("ROW_")
                    .map(|start| (text[start..].to_owned(), rect.min))
            })
            .collect()
    }
    let before = rows(&workbench.frame(vec![]));
    assert!(before.first().unwrap().0[4..].parse::<usize>().unwrap() > 10);
    workbench.click("Maximize");
    workbench.click("Restore");
    assert_eq!(rows(&workbench.frame(vec![])), before);
}
