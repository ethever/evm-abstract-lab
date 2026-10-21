use super::*;
use evm_abstract::{
    Address, U256,
    analysis::{
        control::Cancelled,
        layers::{FactError, FactKey, Observation, SnapshotId, StateFacts},
    },
};

#[test]
fn nested_session_cancellation_uses_the_existing_cli_cancellation_path() {
    let error = CliError::from(RpcAnalysisError::Session(SessionError::Cancelled(
        Cancelled,
    )));
    assert!(matches!(error, CliError::Cancelled(_)));
}

#[test]
fn rejected_fact_refinement_remains_a_typed_session_input_error() {
    let owner = Address::repeat_byte(0x11);
    let snapshot = SnapshotId::offline("CLI fact conflict");
    let facts = StateFacts::new(snapshot.clone())
        .refine(
            &snapshot,
            [Observation::Balance {
                address: owner,
                value: U256::ZERO,
            }],
        )
        .unwrap()
        .into_facts();
    let conflict = facts
        .refine(
            &snapshot,
            [Observation::Balance {
                address: owner,
                value: U256::from(1),
            }],
        )
        .unwrap_err();
    let projected = CliError::from(RpcAnalysisError::Session(SessionError::Facts(conflict)));
    let CliError::Session(SessionError::Facts(FactError::Conflict { key })) = projected else {
        panic!("exact fact failure required");
    };
    assert_eq!(key, FactKey::Balance(owner));
}

#[test]
fn native_engine_and_session_cooperation_preserve_typed_cancellation() {
    let direct = CliError::from(CooperationError::<Infallible>::Cancelled);
    assert!(matches!(direct, CliError::Cancelled(_)));
    let nested = CliError::from(RpcAnalysisError::Session(SessionError::Cooperation(
        CooperationError::<Infallible>::Cancelled,
    )));
    assert!(matches!(nested, CliError::Cancelled(_)));
}
