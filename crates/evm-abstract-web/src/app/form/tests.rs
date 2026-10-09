use super::*;
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

fn frame(ctx: &Context, form: &mut AnalysisForm, events: Vec<Event>) -> Option<AnalyzeRequest> {
    let mut request = None;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..RawInput::default()
        },
        |ui| request = form.show(ui.ctx(), false),
    );
    output.textures_delta.clear();
    request
}
fn enter() -> Event {
    Event::Key {
        key: Key::Enter,
        physical_key: Some(Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL,
    }
}

#[test]
fn opening_focuses_the_real_bytecode_editor_and_submit_preserves_typed_request() {
    let ctx = Context::default();
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    form.bytecode.bytecode.clear();
    frame(&ctx, &mut form, vec![]);
    frame(&ctx, &mut form, vec![]);
    assert!(ctx.text_edit_focused());
    frame(&ctx, &mut form, vec![Event::Text("6001fF".into())]);
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    assert_eq!(
        request.input,
        AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "6001fF".into(),
            address: None
        })
    );
    assert_eq!(request.environment, EnvironmentInput::default());
    assert_eq!(request.limits, AnalysisLimits::default());
    assert!(!form.open);
}

#[test]
fn rpc_form_keeps_snapshot_identity_and_execution_overrides_distinct() {
    let ctx = Context::default();
    let mut form = AnalysisForm {
        open: true,
        source: Source::Rpc,
        ..AnalysisForm::default()
    };
    form.rpc.address = "0x1111111111111111111111111111111111111111".into();
    form.rpc.block = BlockSelector::Hash(format!("0x{}", "12".repeat(32)));
    form.environment.chain_id = Some("0x2".into());
    form.environment.calldata = evm_abstract_protocol::CalldataInput::Exact(String::new());
    frame(&ctx, &mut form, vec![]);
    frame(&ctx, &mut form, vec![]);
    frame(
        &ctx,
        &mut form,
        vec![Event::Text("http://127.0.0.1:8545".into())],
    );
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    let AnalysisInput::Rpc(rpc) = request.input else {
        panic!("expected typed RPC request")
    };
    assert_eq!(rpc.endpoint, "http://127.0.0.1:8545");
    assert_eq!(rpc.block, form.rpc.block);
    assert_eq!(request.environment.chain_id, Some("0x2".into()));
    assert_eq!(
        request.environment.number, None,
        "unchecked header fields inherit the pinned snapshot"
    );
    assert_eq!(
        request.environment.calldata,
        evm_abstract_protocol::CalldataInput::Exact(String::new())
    );
}
