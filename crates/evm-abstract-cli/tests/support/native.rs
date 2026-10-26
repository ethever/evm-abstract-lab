//! Producer identities are opaque across independent CLI processes. Compare
//! reports up to a bijective source renaming, preserving all content and aliasing.
use serde_json::Value as Json;
use std::collections::BTreeMap;

pub fn root(graph: &Json) -> &Json {
    let index = graph["root"]
        .as_u64()
        .expect("native expression has a local root") as usize;
    &graph["nodes"]
        .as_array()
        .expect("native expression is flat")[index]
}

/// The CLI selects two built-in carriers with object payloads. Schema 12 keeps
/// the outer numeric field even when all their sparse components are absent.
pub fn numeric(value: &Json) -> &Json {
    let numeric = value
        .get("numeric")
        .expect("native EVM value requires numeric");
    assert!(
        numeric.is_object(),
        "built-in numeric payload must be an object: {value}"
    );
    numeric
}

/// Compare native evidence independently of requested presentation work, after
/// validating the command's separate, finite postprocessing metadata.
pub fn without_application_metadata(value: &Json) -> Json {
    let mut report = value
        .as_object()
        .expect("application report is an object")
        .clone();
    let model = report
        .remove("numeric_model")
        .expect("selected model is recorded");
    assert!(matches!(model.as_str(), Some("product" | "finite-set")));
    let maximum = report
        .remove("postprocessing_max_work")
        .and_then(|v| v.as_u64())
        .expect("postprocessing allowance is recorded");
    let work = report
        .remove("postprocessing_work")
        .and_then(|v| v.as_u64())
        .expect("postprocessing consumption is recorded");
    assert_eq!(maximum, 2_000_000_000);
    assert!(work <= maximum);
    report.remove("hybrid");
    report.remove("global_status");
    Json::Object(report)
}

pub fn canonical(value: &Json) -> Json {
    let mut value = value.clone();
    rename(&mut value, None, &mut BTreeMap::new(), &mut BTreeMap::new());
    value
}
fn rename(
    value: &mut Json,
    inherited: Option<&str>,
    scopes: &mut BTreeMap<String, usize>,
    fresh: &mut BTreeMap<(String, u64), usize>,
) {
    match value {
        Json::Object(object) => {
            let scope = object
                .get("identity_scope")
                .and_then(Json::as_str)
                .map(str::to_owned);
            if let Some(scope) = &scope {
                let next = scopes.len();
                let id = *scopes.entry(scope.clone()).or_insert(next);
                object.insert(
                    "identity_scope".into(),
                    Json::String(format!("producer-{id}")),
                );
            }
            let context = scope.as_deref().or(inherited);
            if let Some(id) = object.get_mut("Fresh") {
                let original = id.as_u64().expect("fresh token is numeric");
                let next = fresh.len();
                let key = (
                    context
                        .expect("fresh node belongs to a scoped graph")
                        .to_owned(),
                    original,
                );
                *id = Json::from(*fresh.entry(key).or_insert(next));
            }
            for child in object.values_mut() {
                rename(child, context, scopes, fresh);
            }
        }
        Json::Array(values) => {
            for child in values {
                rename(child, inherited, scopes, fresh);
            }
        }
        _ => {}
    }
}

#[test]
fn renaming_preserves_shared_and_independent_fresh_sources() {
    let first = serde_json::json!({"identity_scope":"process-a", "nodes":[{"Fresh":17},{"Fresh":17},{"Fresh":18}]});
    let renamed = serde_json::json!({"identity_scope":"process-b", "nodes":[{"Fresh":91},{"Fresh":91},{"Fresh":92}]});
    let mixed = serde_json::json!({"identity_scope":"process-b", "nodes":[{"Fresh":91},{"Fresh":92},{"Fresh":92}]});
    assert_eq!(canonical(&first), canonical(&renamed));
    assert_ne!(canonical(&first), canonical(&mixed));
    let graph = |scope: &str, fresh: u64| serde_json::json!({"identity_scope":scope, "root":0, "nodes":[{"Fresh":fresh}]});
    let shared = serde_json::json!([graph("a", 17), graph("a", 17)]);
    let independent = serde_json::json!([graph("b", 91), graph("b", 92)]);
    let renamed = serde_json::json!([graph("b", 91), graph("b", 91)]);
    assert_eq!(canonical(&shared), canonical(&renamed));
    assert_ne!(
        canonical(&shared),
        canonical(&independent),
        "separate graphs/outcomes must use the same source bijection"
    );
}

#[test]
fn sparse_components_require_an_object_valued_outer_numeric_payload() {
    for payload in [
        serde_json::json!({}),
        serde_json::json!({"constants":["0x1"]}),
    ] {
        let value = serde_json::json!({"numeric":payload});
        assert_eq!(numeric(&value), &payload);
    }
}

#[test]
#[should_panic(expected = "native EVM value requires numeric")]
fn numeric_requires_outer_payload() {
    numeric(&serde_json::json!({}));
}

#[test]
#[should_panic(expected = "built-in numeric payload must be an object")]
fn numeric_rejects_null_payload() {
    numeric(&serde_json::json!({"numeric":null}));
}

#[test]
#[should_panic(expected = "built-in numeric payload must be an object")]
fn numeric_rejects_legacy_top_string() {
    numeric(&serde_json::json!({"numeric":"Top"}));
}

#[test]
#[should_panic(expected = "native EVM value requires numeric")]
fn numeric_rejects_flattened_constants() {
    numeric(&serde_json::json!({"Constants":["0x1"]}));
}

#[test]
fn application_metadata_preserves_native_evidence() {
    let native = serde_json::json!({"schema_version":13,"work":3,"status":"Incomplete"});
    let mut application = native.clone();
    application["numeric_model"] = serde_json::json!("finite-set");
    application["postprocessing_max_work"] = serde_json::json!(2_000_000_000_u64);
    application["postprocessing_work"] = serde_json::json!(17);
    assert_eq!(without_application_metadata(&application), native);
}

#[test]
#[should_panic(expected = "assertion failed: work <= maximum")]
fn application_metadata_rejects_excessive_consumption() {
    let excessive = serde_json::json!({
        "schema_version":13,"work":3,"status":"Incomplete",
        "numeric_model":"finite-set",
        "postprocessing_max_work":2_000_000_000_u64,
        "postprocessing_work":2_000_000_001_u64,
    });
    without_application_metadata(&excessive);
}

#[test]
#[should_panic(expected = "postprocessing allowance is recorded")]
fn application_metadata_requires_postprocessing_allowance() {
    let missing = serde_json::json!({
        "schema_version":13,"work":3,"status":"Incomplete",
        "numeric_model":"finite-set","postprocessing_work":17,
    });
    without_application_metadata(&missing);
}
/// A frozen physical CLI can be supplied by the evidence runner; ordinary
/// Cargo tests retain Cargo's exact package binary as their default.
pub fn binary() -> std::ffi::OsString {
    std::env::var_os("EVM_ABSTRACT_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_evm-abstract").into())
}

/// Inspect the retained Native namespace after checking the real command's
/// global status separately. Companion SSA and acquisition data keep their
/// source qualifiers; this helper never combines either graph's state IDs.
#[allow(
    dead_code,
    reason = "shared support module is also compiled by native-only integration targets"
)]
pub fn select(report: &Json) -> Json {
    if report.get("global").is_none() {
        assert!(
            report.get("hybrid").is_none() || report.get("global_status").is_some(),
            "Hybrid evidence requires its Global graph"
        );
        return report.clone();
    }
    let hybrid = report.get("hybrid").expect("Hybrid coverage is recorded");
    let graph = report
        .get("native")
        .filter(|graph| graph.is_object())
        .expect("Native evidence is absent; inspect native::global explicitly");
    let native = &graph["analysis"];
    assert!(global(report)["states"].is_array());
    assert!(native["states"].is_array());
    assert!(matches!(
        report["selected_view"].as_str(),
        Some("Native" | "Baseline")
    ));
    assert!(hybrid.get("baseline").is_none() && hybrid.get("detail").is_none());

    let coverage = hybrid
        .get("precision")
        .filter(|value| !value.is_null())
        .unwrap_or(&hybrid["coverage"]);
    for prefix in hybrid["local_detail"].as_array().unwrap() {
        assert_eq!(prefix["namespace"], "Native");
        assert_eq!(prefix["binding"], coverage["detail_binding"]);
        let id = prefix["state_id"].as_u64().unwrap();
        assert!(
            native["states"]
                .as_array()
                .unwrap()
                .iter()
                .any(|state| state["id"] == id)
        );
        assert_eq!(
            prefix["executed_pcs"].as_array().unwrap().len(),
            prefix["instructions"].as_array().unwrap().len()
        );
    }
    let mut native = native.clone();
    if let Some(acquisition) = hybrid.get("rpc").filter(|value| !value.is_null()) {
        native["rpc_acquisition"] = acquisition["acquisition"].clone();
    }
    let has_ssa = graph.get("ssa").is_some() || graph.get("partial_ssa").is_some();
    let mut result = if has_ssa {
        let mut focused = graph.clone();
        focused["analysis"] = native;
        focused["global_status"] = report["status"].clone();
        focused["status"] = focused["analysis"]["status"].clone();
        focused
    } else {
        native
    };
    for key in [
        "numeric_model",
        "postprocessing_max_work",
        "postprocessing_work",
    ] {
        result[key] = report[key].clone();
    }
    result["hybrid"] = hybrid.clone();
    result["global_status"] = report["status"].clone();

    result
}

/// Global control locations describe a MAY model. Borrow its analysis explicitly
/// so a missing Native slot can never become executed-PC or SSA evidence.
pub fn global(report: &Json) -> &Json {
    assert!(report["hybrid"].is_object(), "Hybrid coverage is recorded");
    let analysis = &report["global"]["analysis"];
    assert!(analysis["states"].is_array(), "Global analysis is recorded");
    analysis
}

#[test]
fn hybrid_namespaces_keep_colliding_state_ids_and_ssa_separate() {
    let report = serde_json::json!({
        "global": {"analysis": {"states": [{"id": 0, "executed_pcs": []}]}},
        "native": {
            "analysis": {"status": "Converged", "states": [{"id": 0, "executed_pcs": [7]}]},
            "ssa": {"instructions": [{"pc": 7}]}
        },
        "hybrid": {"coverage": {"detail_binding": "OriginalOwner"}, "local_detail": []},
        "selected_view": "Baseline", "status": "Converged"
    });
    assert_eq!(
        global(&report)["states"][0]["executed_pcs"],
        serde_json::json!([])
    );
    let native = select(&report);
    assert_eq!(
        native["analysis"]["states"][0]["executed_pcs"],
        serde_json::json!([7])
    );
    assert_eq!(native["ssa"]["instructions"][0]["pc"], 7);
    assert_eq!(select(&native), native);
}

#[test]
#[should_panic(expected = "Native evidence is absent; inspect native::global explicitly")]
fn absent_native_evidence_cannot_fall_back_to_global_control() {
    let report = serde_json::json!({
        "global": {"analysis": {"states": [{"id": 0, "executed_pcs": []}]}},
        "native": null,
        "hybrid": {"local_detail": []}, "selected_view": "Baseline"
    });
    select(&report);
}

#[test]
#[should_panic(expected = "Hybrid evidence requires its Global graph")]
fn hybrid_evidence_requires_its_graph_envelope() {
    select(&serde_json::json!({"hybrid": {}, "native": null}));
}

#[allow(
    dead_code,
    reason = "shared support module is also compiled by native-only integration targets"
)]
pub fn assert_exit(report: &Json, output: &std::process::Output) {
    let status = report
        .get("global_status")
        .or_else(|| {
            report
                .get("machine_analysis")
                .map(|machine| &machine["status"])
        })
        .unwrap_or(&report["status"]);
    assert_eq!(
        output.status.code(),
        Some(if status == "Converged" { 0 } else { 2 }),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(hybrid) = report.get("hybrid") {
        assert_eq!(
            status == "Converged",
            hybrid["coverage"]["baseline_current"] == true
        );
    }
}

/// Text preserves independent global closure and pending native detail.
#[allow(
    dead_code,
    reason = "shared support module is also compiled by native-only integration targets"
)]
pub fn text_exit(output: &std::process::Output, native_exit: i32) {
    let text = String::from_utf8_lossy(&output.stdout);
    let expected = if text.contains("Global coverage: Converged") {
        0
    } else {
        native_exit
    };
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
