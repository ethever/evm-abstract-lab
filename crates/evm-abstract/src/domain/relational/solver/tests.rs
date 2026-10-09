//! Provider failures and UNKNOWN have different semantics and wire identities.

use super::{CheckResult, QueryReason, RelationLimits, checked, unknown};
use crate::domain::relational::{SmtError, SmtProvider};
use embedded_smt::Unknown;

#[test]
fn real_provider_configuration_failures_remain_errors_with_provider_identity() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            rlimit: 0,
            ..RelationLimits::default()
        };
        let result = checked(&[], &limits);
        let CheckResult::Unknown(QueryReason::SolverError(error)) = &result else {
            panic!("a rejected provider call must not become native UNKNOWN: {result:?}");
        };
        assert_eq!(error.provider, provider);
        assert_eq!(error.message, "rlimit must be positive");
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["Unknown"]["SolverError"]["provider"],
            provider.to_string()
        );
        assert_eq!(
            json["Unknown"]["SolverError"]["message"],
            "rlimit must be positive"
        );
        assert!(json["Unknown"]["SolverUnknown"].is_null());
    }
}

#[test]
fn identical_native_text_cannot_merge_unknown_with_a_binding_error_or_other_provider() {
    let explanation = "native explanation with no encoded provider or category";
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let undecided = unknown(Unknown::Solver(explanation.into()), provider);
        let failed = QueryReason::SolverError(SmtError {
            provider,
            message: explanation.into(),
        });
        assert_ne!(undecided, failed);
        let json = serde_json::to_value(&undecided).unwrap();
        assert_eq!(json["SolverUnknown"]["provider"], provider.to_string());
        assert_eq!(json["SolverUnknown"]["reason"], explanation);
    }
}
