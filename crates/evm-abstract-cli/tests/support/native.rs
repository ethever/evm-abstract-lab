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
    for malformed in [
        serde_json::json!({}),
        serde_json::json!({"numeric":null}),
        serde_json::json!({"numeric":"Top"}),
        serde_json::json!({"Constants":["0x1"]}),
    ] {
        assert!(std::panic::catch_unwind(|| numeric(&malformed)).is_err());
    }
}

#[test]
fn application_metadata_preserves_native_evidence_and_rejects_invalid_consumption() {
    let native = serde_json::json!({"schema_version":12,"work":3,"status":"Incomplete"});
    let mut application = native.clone();
    application["numeric_model"] = serde_json::json!("finite-set");
    application["postprocessing_max_work"] = serde_json::json!(2_000_000_000_u64);
    application["postprocessing_work"] = serde_json::json!(17);
    assert_eq!(without_application_metadata(&application), native);
    let mut excessive = application.clone();
    excessive["postprocessing_work"] = serde_json::json!(2_000_000_001_u64);
    assert!(std::panic::catch_unwind(|| without_application_metadata(&excessive)).is_err());
    application
        .as_object_mut()
        .unwrap()
        .remove("postprocessing_max_work");
    assert!(std::panic::catch_unwind(|| without_application_metadata(&application)).is_err());
}
