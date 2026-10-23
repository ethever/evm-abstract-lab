//! 用真实二进制核对默认组合域、对照参数、冻结 JSON 策略与入口资源停止。

use super::{analyze, run_concrete};
use serde_json::Value;

fn cfg(code: &str, extra: &[&str]) -> Value {
    let mut args = vec!["cfg", "--hex", code, "--format", "json", "--no-relations"];
    args.extend_from_slice(extra);
    let output = run_concrete(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn product_and_finite_set_use_distinct_native_carriers_and_precision() {
    let product = cfg("5f3560011660021600", &["--max-constants", "1"]);
    let finite = cfg(
        "5f3560011660021600",
        &["--max-constants", "1", "--numeric-model", "finite-set"],
    );
    assert_eq!(product["schema_version"], 14);
    assert_eq!(product["numeric_model"], "product");
    assert_eq!(finite["numeric_model"], "finite-set");
    assert_eq!(
        product["states"][0]["exit_stack"][0]["numeric"]["constants"],
        serde_json::json!(["0x0"])
    );
    assert_eq!(
        finite["states"][0]["exit_stack"][0]["numeric"],
        serde_json::json!({}),
        "the finite carrier must not be a product value with precision switched off",
    );
    for json in [&product, &finite] {
        assert_eq!(json["status"], "Converged");
        assert_eq!(json["domain_spec"]["word_bits"], 256);
        assert_eq!(
            json["domain_spec"]["cost_version"],
            evm_abstract::domain::DomainSpec::default().cost_version()
        );
        assert!(json["domain_spec"].get("profile").is_none());
        for field in [
            "domain_profile",
            "numeric_model",
            "max_constants",
            "reduction_rounds",
            "max_facts",
            "relations",
        ] {
            assert!(json["config"].get(field).is_none(), "{field}");
        }
        assert_eq!(json["domain_spec"]["widening_after_updates"], 2);
        assert_eq!(
            json["domain_spec"]["provenance_policy"],
            "scoped-expressions-and-value-identities-v3"
        );
    }
}

#[test]
fn explicit_fact_policy_is_reported_and_preserves_component_constraints() {
    let json = cfg(
        "5f351500",
        &[
            "--numeric-model",
            "product",
            "--max-constants",
            "1",
            "--reduction-rounds",
            "1",
            "--max-facts",
            "1",
        ],
    );
    assert!(json["config"].get("reduction_rounds").is_none());
    assert!(json["config"].get("max_facts").is_none());
    assert_eq!(json["domain_spec"]["reduction_rounds"], 1);
    assert_eq!(json["domain_spec"]["fact_limit"], 1);
    let boolean = &json["states"][0]["exit_stack"][0];
    assert!(boolean.is_object());
    assert!(crate::native::numeric(boolean).get("congruence").is_none());
    assert_eq!(boolean["numeric"]["interval"]["unsigned_lo"], "0x0");
    assert_eq!(boolean["numeric"]["interval"]["unsigned_hi"], "0x1");
    assert!(crate::native::numeric(boolean).get("constants").is_none());
    assert!(json["frontiers"].as_array().unwrap().is_empty());
}

#[test]
fn zero_fact_policy_is_rejected_before_execution() {
    for flag in ["--reduction-rounds", "--max-facts"] {
        let output = run_concrete(&["cfg", "--hex", "00", flag, "0", "--format", "json"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be positive"));
    }
    assert_eq!(
        run_concrete(&["cfg", "--hex", "00", "--numeric-model", "unknown"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn initial_world_budget_is_incomplete_with_an_explicit_source_free_frontier() {
    let output = analyze(
        "call-return-branch",
        &[
            "--format",
            "json",
            "--max-work",
            "1",
            "--no-root-terminal-coverage",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "Incomplete");
    assert!(json["states"].as_array().unwrap().is_empty());
    assert!(json["outcomes"].as_array().unwrap().is_empty());
    let frontiers = json["frontiers"].as_array().unwrap();
    assert_eq!(frontiers.len(), 1);
    assert!(frontiers[0]["from"].is_null());
    assert!(frontiers[0]["target"].is_null());
    assert!(frontiers[0]["pc"].is_null());
    assert_eq!(frontiers[0]["reason"], "Work");
    assert_eq!(json["schema_version"], 14);
    assert_eq!(json["numeric_model"], "product");
    assert_eq!(json["postprocessing_max_work"], 2_000_000_000);
    assert_eq!(json["postprocessing_work"], 0);
}
