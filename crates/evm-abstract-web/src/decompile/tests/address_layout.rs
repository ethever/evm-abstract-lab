//! Address source editors must have their own visible allocation at every width.
use super::{Command, Message, Workspace, api};
use crate::controls;
use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

fn frame(
    context: &Context,
    workspace: &mut Workspace,
    size: Vec2,
    events: Vec<Event>,
) -> (Option<Command>, Vec<serde_json::Value>) {
    let mut command = None;
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..Default::default()
    };
    workspace.prepare_input(&mut input);
    let mut output = context.run_ui(input, |ui| {
        controls::begin_observation(ui.ctx(), true);
        command = workspace.show(ui);
    });
    output.textures_delta.clear();
    (
        command,
        serde_json::from_str(&controls::observed(context).unwrap()).unwrap(),
    )
}

fn editor(context: &Context, rows: &[serde_json::Value], size: Vec2, label: &str) -> Rect {
    let record = rows
        .iter()
        .find(|row| row["label"] == label && row["role"] == "text")
        .unwrap_or_else(|| panic!("{label} is not visible at {size:?}: {rows:?}"));
    assert_eq!(record["enabled"], true);
    let full = context.read_response(egui::Id::new(label)).unwrap().rect;
    assert!(
        Rect::from_min_size(Pos2::ZERO, size).contains_rect(full),
        "{label} extends beyond {size:?}: {full:?}"
    );
    assert!(
        full.width() >= 120.0 && full.height() >= 18.0,
        "{label} has no usable editor allocation: {full:?}"
    );
    full
}

fn edit(context: &Context, workspace: &mut Workspace, size: Vec2, label: &str, value: &str) {
    let (_, rows) = frame(context, workspace, size, vec![]);
    let position = editor(context, &rows, size, label).center();
    for pressed in [true, false] {
        frame(
            context,
            workspace,
            size,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    assert!(
        context
            .read_response(egui::Id::new(label))
            .unwrap()
            .has_focus()
    );
    let mut events = Vec::new();
    for (key, modifiers) in [
        (
            Key::A,
            Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            },
        ),
        (Key::Backspace, Modifiers::NONE),
    ] {
        for pressed in [true, false] {
            events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            });
        }
    }
    events.push(Event::Paste(value.into()));
    frame(context, workspace, size, events);
}

#[test]
fn block_and_chain_inputs_are_visible_and_editable_in_wide_and_narrow_workspaces() {
    for size in [Vec2::new(1920.0, 1080.0), Vec2::new(390.0, 844.0)] {
        let context = Context::default();
        crate::palette::configure(&context);
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut workspace = Workspace::default();
        workspace.form.load_address(&api::DecompileAddressInput {
            address: format!("0x{}", "12".repeat(20)),
            provider_id: None,
            block: api::BlockSelector::Latest,
            chain_id: Some("1".into()),
        });
        let Command::Providers { generation } = workspace.provider_command().unwrap() else {
            unreachable!()
        };
        workspace.receive(Message::Providers {
            generation,
            result: Ok(api::RpcProvidersReply {
                result: Ok(vec![api::RpcProvider {
                    id: "chain".into(),
                    name: "Local chain".into(),
                    endpoint: "http://localhost/".into(),
                }]),
            }),
        });
        for _ in 0..3 {
            frame(&context, &mut workspace, size, vec![]);
        }
        let (_, rows) = frame(&context, &mut workspace, size, vec![]);
        let block = editor(&context, &rows, size, "Source block");
        let chain = editor(&context, &rows, size, "Expected chain ID");
        assert!(
            !block.intersects(chain),
            "source editors must own separate geometry"
        );
        edit(&context, &mut workspace, size, "Source block", "42");
        edit(&context, &mut workspace, size, "Expected chain ID", "10");
        assert!(
            workspace.form.validation().is_none(),
            "edited source is invalid: {:?}",
            workspace.form.validation()
        );
        let (command, _) = frame(
            &context,
            &mut workspace,
            size,
            vec![Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::CTRL,
            }],
        );
        let Some(Command::Open { request, .. }) = command else {
            panic!("edited address must submit at {size:?}")
        };
        let api::DecompileSourceInput::Address(address) = request.source else {
            panic!("address input mode changed")
        };
        assert_eq!(address.block, api::BlockSelector::Number(42));
        assert_eq!(address.chain_id.as_deref(), Some("10"));
    }
}
