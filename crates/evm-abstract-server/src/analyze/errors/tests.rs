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
    let control = Control::new(Default::default());
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

#[test]
fn session_cancellation_is_cancelled_before_report_generation() {
    use evm_abstract::analysis::{RpcAnalysisError, control::Cancelled, layers::SessionError};
    let projected = super::super::rpc_error(RpcAnalysisError::Session(SessionError::Cancelled(
        Cancelled,
    )));
    assert_eq!(projected.code, api::ApiErrorCode::Cancelled);
    assert!(matches!(
        projected.details,
        api::ErrorDetails::Task(api::TaskFailure {
            kind: api::TaskErrorKind::Cancelled,
            id: None
        })
    ));
    roundtrip(&projected);
}

#[test]
fn rejected_initial_storage_fact_retains_exact_owner_and_full_slot_key() {
    use evm_abstract::analysis::{
        RpcAnalysisError,
        layers::{Observation, SessionError, SnapshotId, StateFacts},
    };
    let owner = Address::repeat_byte(0x11);
    let slot = U256::MAX;
    let snapshot = SnapshotId::offline("storage observation conflict");
    let facts = StateFacts::new(snapshot.clone())
        .refine(
            &snapshot,
            [Observation::Storage {
                address: owner,
                slot,
                value: U256::ZERO,
            }],
        )
        .unwrap()
        .into_facts();
    let conflict = facts
        .refine(
            &snapshot,
            [Observation::Storage {
                address: owner,
                slot,
                value: U256::from(1),
            }],
        )
        .unwrap_err();
    let projected =
        super::super::rpc_error(RpcAnalysisError::Session(SessionError::Facts(conflict)));
    assert_eq!(projected.code, api::ApiErrorCode::InvalidRequest);
    assert_eq!(
        projected.details,
        api::ErrorDetails::Session(Box::new(api::SessionFailure::FactConflict(
            api::InitialFactLocation::Storage(api::StorageLocation {
                address: owner.to_string(),
                slot: format!("0x{slot:x}")
            })
        )))
    );
    roundtrip(&projected);
}

#[test]
fn independent_offline_snapshots_with_equal_labels_still_report_snapshot_mismatch() {
    use evm_abstract::analysis::{
        RpcAnalysisError,
        layers::{SessionError, SnapshotId, StateFacts},
    };
    let expected = SnapshotId::offline("same label");
    let observed = SnapshotId::offline("same label");
    assert_ne!(expected, observed);
    let mismatch = StateFacts::new(expected).refine(&observed, []).unwrap_err();
    let projected =
        super::super::rpc_error(RpcAnalysisError::Session(SessionError::Facts(mismatch)));
    assert_eq!(projected.code, api::ApiErrorCode::InvalidRequest);
    assert_eq!(
        projected.details,
        api::ErrorDetails::Session(Box::new(api::SessionFailure::Snapshot(
            api::SessionSnapshotMismatch {
                expected: api::SessionSnapshotDescription::Offline("same label".into()),
                observed: api::SessionSnapshotDescription::Offline("same label".into()),
            }
        )))
    );
    roundtrip(&projected);
}
