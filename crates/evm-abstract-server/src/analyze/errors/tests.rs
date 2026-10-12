use super::*;
use evm_abstract::{
    Address, Fork, U256,
    analysis::control::Control,
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprLimits},
    },
    world::{Account, World},
};

fn world(expression: ExprId) -> World {
    let mut world = World::new(Fork::Osaka, "typed fingerprint failure");
    let mut account = Account::empty();
    account.balance = AbstractValue::top().with_expression(expression);
    world.insert(Address::repeat_byte(0x11), account).unwrap();
    world
}

fn roundtrip(error: &api::ApiError) {
    let decoded: api::ApiError =
        serde_json::from_slice(&serde_json::to_vec(error).unwrap()).unwrap();
    assert_eq!(&decoded, error);
}

#[test]
fn cancelled_real_fingerprint_stays_cancelled_in_the_protocol_error() {
    let world = world(ExprId::fresh().unwrap());
    let previous = world.fingerprint().unwrap();
    let control = Control::default();
    control.cancellation().cancel();
    let error = control.scope(|| fingerprint(world.fingerprint().unwrap_err()));
    assert_eq!(error.code, api::ApiErrorCode::Cancelled);
    assert!(matches!(
        error.details,
        api::ErrorDetails::Fingerprint(api::FingerprintFailure {
            kind: api::FingerprintFailureKind::Cancelled,
            line: None,
            column: None,
        })
    ));
    roundtrip(&error);
    assert_eq!(world.fingerprint().unwrap(), previous);
}

#[test]
fn actual_graph_encoding_boundary_retains_its_finite_typed_cause() {
    let mut expression = ExprId::fresh().unwrap();
    for _ in 0..257 {
        expression = ExprId::operation(
            0x01, // ADD
            &[expression, ExprId::constant(U256::from(1))],
            ExprLimits {
                max_nodes: 65_536,
                max_depth: 512,
            },
        )
        .unwrap();
    }
    let error = fingerprint(world(expression).fingerprint().unwrap_err());
    assert_eq!(error.code, api::ApiErrorCode::Export);
    assert!(matches!(
        error.details,
        api::ErrorDetails::Fingerprint(api::FingerprintFailure {
            kind: api::FingerprintFailureKind::Encoding,
            line: Some(_),
            column: Some(_),
        })
    ));
    assert!(serde_json::to_vec(&error).unwrap().len() < 1024);
    roundtrip(&error);
}
