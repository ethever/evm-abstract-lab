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
