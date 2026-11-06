use crate::{controls, decompile::form::Form};
use egui::{Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{DecompileRequest, Fork};
use serde_json::Value;

const BUDGETS: [(&str, &str); 6] = [
    ("Work limit", "max_work"),
    ("Block limit", "max_blocks"),
    ("Operation limit", "max_operations"),
    ("Clone limit", "max_cloned_blocks"),
    ("History limit", "max_history_entities"),
    ("Recovery iteration limit", "max_iterations"),
];

struct Frame {
    request: Option<DecompileRequest>,
    controls: Vec<Value>,
    output: FullOutput,
}

impl Frame {
    fn control(&self, label: &str, role: &str) -> &Value {
        self.controls
            .iter()
            .find(|control| control["label"] == label && control["role"] == role)
            .unwrap_or_else(|| panic!("missing {role} {label}: {:?}", self.controls))
    }

    fn text_count(&self, expected: &str) -> usize {
        fn count(shape: &egui::Shape, expected: &str) -> usize {
            match shape {
                egui::Shape::Text(text) => usize::from(text.galley.text() == expected),
                egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, expected)).sum(),
                _ => 0,
            }
        }
        self.output
            .shapes
            .iter()
            .map(|shape| count(&shape.shape, expected))
            .sum()
    }
}

/// Reuse the workbench's real response observations and browser-shaped input;
/// no test reaches into NumberInputs or mutates its private draft state.
struct Editor {
    context: Context,
    form: Form,
    busy: bool,
}

impl Editor {
    fn new() -> Self {
        let context = Context::default();
        crate::palette::configure(&context);
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut editor = Self {
            context,
            form: Form::default(),
            busy: false,
        };
        editor.form.load("00".into(), Fork::Osaka);
        editor.frame(vec![]);
        editor.frame(vec![]);
        editor.click("Decompile budgets", "section");
        editor.frame(vec![]);
        editor
    }

    fn frame(&mut self, events: Vec<Event>) -> Frame {
        let mut request = None;
        let mut output = self.context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                request = self.form.show(ui, self.busy);
            },
        );
        output.textures_delta.clear();
        Frame {
            request,
            controls: serde_json::from_str(&controls::observed(&self.context).unwrap()).unwrap(),
            output,
        }
    }

    fn click(&mut self, label: &str, role: &str) -> Frame {
        let frame = self.frame(vec![]);
        let [left, top, right, bottom]: [f32; 4] =
            serde_json::from_value(frame.control(label, role)["rect"].clone()).unwrap();
        let pos = Pos2::new((left + right) / 2.0, (top + bottom) / 2.0);
        let pointer = |pressed| {
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]
        };
        assert!(self.frame(pointer(true)).request.is_none());
        self.frame(pointer(false))
    }

    fn replace(&mut self, label: &str, text: &str) -> Frame {
        assert!(self.click(label, "text").request.is_none());
        let frame = self.frame(replace_events(text));
        assert!(frame.request.is_none(), "editing must not submit");
        frame
    }

    fn shortcut(&mut self) -> Frame {
        self.frame(key(Key::Enter, Modifiers::CTRL))
    }

    fn assert_blocked(&mut self) {
        let frame = self.frame(vec![]);
        assert_eq!(frame.control("Decompile", "action")["enabled"], false);
        assert!(self.shortcut().request.is_none());
        assert!(self.click("Decompile", "action").request.is_none());
    }
}

fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

fn replace_events(text: &str) -> Vec<Event> {
    let mut events = key(
        Key::A,
        Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        },
    );
    events.extend(key(Key::Backspace, Modifiers::NONE));
    events.push(Event::Paste(text.into()));
    events
}

fn wire(request: &DecompileRequest) -> (String, Value) {
    let text = serde_json::to_string(request).unwrap();
    let decoded = serde_json::from_str::<DecompileRequest>(&text).unwrap();
    assert_eq!(&decoded, request);
    let value = serde_json::from_str(&text).unwrap();
    (text, value)
}

#[test]
fn six_budget_editors_show_and_submit_exact_shared_maximum_defaults() {
    let mut editor = Editor::new();
    let frame = editor.frame(vec![]);
    assert_eq!(frame.text_count("18446744073709551615"), 6);
    for (label, _) in BUDGETS {
        assert_eq!(frame.control(label, "text")["enabled"], true);
        assert!(
            frame
                .controls
                .iter()
                .all(|control| control["role"] != "number")
        );
    }
    let request = editor
        .click("Decompile", "action")
        .request
        .expect("valid default budgets enable the real button");
    assert_eq!(request.limits, Default::default());
    assert_eq!(request.limits.max_constant_set, 8);
    assert_eq!(request.limits.max_structuring_depth, 512);
    let (text, value) = wire(&request);
    for (_, field) in BUDGETS {
        assert!(text.contains(&format!("\"{field}\":18446744073709551615")));
        assert_eq!(value["limits"][field].as_u64(), Some(u64::MAX));
    }
}

#[test]
fn every_budget_accepts_wide_integers_and_serializes_raw_numeric_tokens_exactly() {
    let mut editor = Editor::new();
    for (label, field) in BUDGETS {
        for number in [
            4_294_967_296_u64,
            10_000_000_000,
            9_007_199_254_740_993,
            u64::MAX,
        ] {
            let digits = number.to_string();
            let frame = editor.replace(label, &digits);
            assert!(frame.text_count(&digits) > 0, "{label}: {digits}");
            let request = editor.shortcut().request.expect("valid budget submits");
            let (text, value) = wire(&request);
            assert!(text.contains(&format!("\"{field}\":{digits}")), "{text}");
            assert_eq!(value["limits"][field].as_u64(), Some(number), "{label}");
        }
    }
}

#[test]
fn budget_labels_bind_independently_and_valid_draft_digits_survive_submission() {
    let mut editor = Editor::new();
    let drafts = [
        "0009007199254740993",
        "4294967296",
        "10000000000",
        "18446744073709551615",
        "0",
        "0000000000000000007",
    ];
    for ((label, _), draft) in BUDGETS.into_iter().zip(drafts) {
        assert!(editor.replace(label, draft).text_count(draft) > 0);
    }
    for request in [
        editor.shortcut().request,
        editor.click("Decompile", "action").request,
    ] {
        let (text, value) = wire(&request.unwrap());
        for ((_, field), draft) in BUDGETS.into_iter().zip(drafts) {
            let number = draft.parse::<u64>().unwrap();
            assert_eq!(value["limits"][field].as_u64(), Some(number));
            assert!(text.contains(&format!("\"{field}\":{number}")));
        }
    }
    let frame = editor.frame(vec![]);
    for draft in drafts {
        assert!(
            frame.text_count(draft) > 0,
            "draft spelling changed: {draft}"
        );
    }
}

#[test]
fn overflow_in_each_budget_blocks_button_and_shortcut_until_corrected() {
    let mut editor = Editor::new();
    for (label, field) in BUDGETS {
        let frame = editor.replace(label, "18446744073709551616");
        assert!(frame.text_count("18446744073709551616") > 0);
        assert!(frame.text_count("Enter a supported unsigned whole number") > 0);
        assert!(
            editor
                .form
                .status()
                .contains("Correct invalid decompile budgets")
        );
        editor.assert_blocked();
        editor.replace(label, "9007199254740993");
        let request = editor.click("Decompile", "action").request.unwrap();
        assert_eq!(
            wire(&request).1["limits"][field].as_u64(),
            Some(9_007_199_254_740_993)
        );
    }
}

#[test]
fn blank_negative_fractional_and_nonnumeric_drafts_never_submit_previous_valid_limits() {
    let mut editor = Editor::new();
    for invalid in ["", "-1", "1.5", "not a number", "1e3"] {
        editor.replace("Work limit", "4294967296");
        let frame = editor.replace("Work limit", invalid);
        assert!(frame.text_count("Enter a supported unsigned whole number") > 0);
        editor.assert_blocked();
        editor.replace("Work limit", "18446744073709551615");
        let request = editor.shortcut().request.unwrap();
        assert_eq!(request.limits.max_work, u64::MAX);
    }
}

#[test]
fn invalid_edit_and_submit_chord_in_the_same_frame_never_send_the_old_budget() {
    let mut editor = Editor::new();
    for draft in ["18446744073709551616", ""] {
        editor.replace("Work limit", "9007199254740993");
        let mut events = replace_events(draft);
        events.extend(key(Key::Enter, Modifiers::CTRL));
        let frame = editor.frame(events);
        assert!(frame.request.is_none());
        assert!(frame.text_count("Enter a supported unsigned whole number") > 0);
        editor.assert_blocked();
    }
}

#[test]
fn hide_load_and_collapsed_budgets_preserve_both_valid_and_invalid_preferences() {
    let mut editor = Editor::new();
    editor.replace("Work limit", "0009007199254740993");
    editor.replace("Block limit", "18446744073709551616");
    editor.replace("Operation limit", "-1");
    assert!(editor.click("Hide input", "action").request.is_none());
    assert!(!editor.form.open);
    assert!(editor.shortcut().request.is_none());
    editor.form.load("0X60 01 00".into(), Fork::Prague);
    let frame = editor.frame(vec![]);
    for draft in ["0009007199254740993", "18446744073709551616", "-1"] {
        assert!(frame.text_count(draft) > 0, "load lost preference {draft}");
    }
    assert_eq!(
        editor.context.memory(|memory| memory.focused()),
        Some(crate::framework::BytecodeEditor::Decompiler.id())
    );
    editor.assert_blocked();
    editor.click("Decompile budgets", "section");
    editor.assert_blocked();
    editor.click("Decompile budgets", "section");
    editor.replace("Block limit", "10000000000");
    editor.assert_blocked();
    editor.replace("Operation limit", "4294967296");
    let request = editor.shortcut().request.unwrap();
    assert_eq!(request.bytecode, "0x600100");
    assert_eq!(request.fork, Fork::Prague);
    assert_eq!(request.limits.max_work, 9_007_199_254_740_993);
    assert_eq!(request.limits.max_blocks, 10_000_000_000);
    assert_eq!(request.limits.max_operations, 4_294_967_296);
    assert!(editor.frame(vec![]).text_count("0009007199254740993") > 0);
}

#[test]
fn numeric_enter_commit_does_not_submit_and_busy_or_invalid_bytecode_still_blocks_both_routes() {
    let mut editor = Editor::new();
    let frame = editor.replace("Recovery iteration limit", "9007199254740993");
    let focused = editor.context.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        focused.value().to_string(),
        frame.control("Recovery iteration limit", "text")["id"]
            .as_str()
            .unwrap()
    );
    assert!(
        editor
            .frame(key(Key::Enter, Modifiers::NONE))
            .request
            .is_none()
    );
    editor.click("Recovery iteration limit", "text");
    // egui derives repeat from the held-key state. Feed a real first press
    // before the repeated event rather than marking an initial press as repeat.
    let chord = key(Key::Enter, Modifiers::CTRL);
    assert!(editor.frame(vec![chord[0].clone()]).request.is_some());
    assert!(editor.frame(vec![chord[0].clone()]).request.is_none());
    editor.frame(vec![chord[1].clone()]);
    editor.busy = true;
    editor.assert_blocked();
    editor.busy = false;
    editor.form.load("0x123".into(), Fork::Cancun);
    editor.assert_blocked();
    editor.form.load("00".into(), Fork::Cancun);
    let request = editor.shortcut().request.unwrap();
    assert_eq!(request.limits.max_iterations, 9_007_199_254_740_993);
}

#[test]
fn valid_prefix_and_whitespace_drafts_send_parser_compatible_payloads_without_rewriting_edits() {
    for (draft, expected) in [
        ("0x600160020100", "0x600160020100"),
        (" \n0X60 01\t60 02 01 00 ", "0x600160020100"),
        ("60 01\n60 02 01 00", "600160020100"),
    ] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut form = Form::default();
        form.load(draft.into(), evm_abstract_protocol::Fork::Osaka);
        assert!(form.validation().is_none());
        let mut initial = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 800.0))),
                ..RawInput::default()
            },
            |ui| {
                assert!(form.show(ui, false).is_none());
            },
        );
        initial.textures_delta.clear();
        let mut request = None;
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 800.0))),
                events: vec![Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::CTRL,
                }],
                ..RawInput::default()
            },
            |ui| request = form.show(ui, false),
        );
        output.textures_delta.clear();
        assert_eq!(
            request
                .expect("valid draft submits by the real shortcut")
                .bytecode,
            expected
        );
        assert_eq!(form.bytecode, draft);
    }
}
