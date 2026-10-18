//! Decode only the fixture reader's existing concrete node subset.
//!
//! Consumers already receive `Value`: issue13 parses CLI output with
//! `serde_json::from_slice`, and snapshots use `to_value`. The old `from_value`
//! derives therefore never saw raw duplicate object keys either. Checking exact
//! fields here preserves that boundary without derive-generated dynamic errors.

use super::{Binding, Graph, Json, Node, Source};
use serde::de::DeserializeOwned;

fn decoded<T: DeserializeOwned>(value: &Json) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

fn object_fields<'a, const N: usize>(value: &'a Json, names: [&str; N]) -> Option<[&'a Json; N]> {
    let object = value.as_object()?;
    if object.len() != N {
        return None;
    }
    let mut fields = [value; N];
    for (field, name) in fields.iter_mut().zip(names) {
        *field = object.get(name)?;
    }
    Some(fields)
}

// Ordinary derived structs accepted exact-length sequences through from_value;
// serde_json's externally tagged struct variants accepted only objects.
fn record_fields<'a, const N: usize>(value: &'a Json, names: [&str; N]) -> Option<[&'a Json; N]> {
    match value {
        Json::Array(values) => {
            let fields: &[Json; N] = values.as_slice().try_into().ok()?;
            Some(fields.each_ref())
        }
        _ => object_fields(value, names),
    }
}

pub(super) fn graph(value: &Json) -> Option<Graph> {
    let [identity_scope, root, nodes] = record_fields(value, ["identity_scope", "root", "nodes"])?;
    Some(Graph {
        identity_scope: decoded(identity_scope)?,
        root: decoded(root)?,
        nodes: nodes.as_array()?.iter().map(node).collect::<Option<_>>()?,
    })
}

fn source(value: &Json) -> Option<Source> {
    let [token, namespace] = record_fields(value, ["token", "namespace"])?;
    Some(Source {
        token: decoded(token)?,
        namespace: decoded(namespace)?,
    })
}

fn binding(value: &Json) -> Option<Binding> {
    let [index, value, numeric] = record_fields(value, ["index", "value", "numeric"])?;
    Some(Binding {
        index: decoded(index)?,
        value: decoded(value)?,
        numeric: numeric.clone(),
    })
}

fn node(value: &Json) -> Option<Node> {
    if value.as_str() == Some("BytesZero") {
        return Some(Node::BytesZero);
    }
    let tagged = value.as_object()?;
    if tagged.len() != 1 {
        return None;
    }
    let (tag, payload) = tagged.iter().next()?;
    Some(match tag.as_str() {
        "Constant" => Node::Constant(decoded(payload)?),
        "SlotsInput" => {
            let [input, bindings] = object_fields(payload, ["source", "bindings"])?;
            Node::SlotsInput {
                source: source(input)?,
                bindings: bindings
                    .as_array()?
                    .iter()
                    .map(binding)
                    .collect::<Option<_>>()?,
            }
        }
        "SlotRead" => {
            let [state, key] = object_fields(payload, ["state", "key"])?;
            Node::SlotRead {
                state: decoded(state)?,
                key: decoded(key)?,
            }
        }
        "BytesZero" if payload.is_null() => Node::BytesZero,
        "BytesExact" => Node::BytesExact(decoded(payload)?),
        "WriteWord" => {
            let [previous, offset, value] =
                object_fields(payload, ["previous", "offset", "value"])?;
            Node::WriteWord {
                previous: decoded(previous)?,
                offset: decoded(offset)?,
                value: decoded(value)?,
            }
        }
        "View" => {
            let [state, offset, length] = object_fields(payload, ["state", "offset", "length"])?;
            Node::View {
                state: decoded(state)?,
                offset: decoded(offset)?,
                length: decoded(length)?,
            }
        }
        "Slice" => {
            let [source] = object_fields(payload, ["source"])?;
            Node::Slice {
                source: decoded(source)?,
            }
        }
        "Copy" => {
            let [previous, destination, source, length] =
                object_fields(payload, ["previous", "destination", "source", "length"])?;
            Node::Copy {
                previous: decoded(previous)?,
                destination: decoded(destination)?,
                source: decoded(source)?,
                length: decoded(length)?,
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        super::{Json, Node, U256},
        binding, graph, node, source,
    };
    use serde_json::json;

    #[test]
    fn graph_requires_exact_fields_and_types() {
        let valid = json!({"identity_scope": "capture", "root": 0, "nodes": ["BytesZero"]});
        assert!(graph(&valid).is_some());
        for name in ["identity_scope", "root", "nodes"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(name);
            assert!(graph(&missing).is_none());
            let mut null = valid.clone();
            null[name] = Json::Null;
            assert!(graph(&null).is_none());
        }
        let mut unknown = valid.clone();
        unknown["extra"] = Json::Null;
        assert!(graph(&unknown).is_none());
        for root in [json!(-1), json!(0.0), json!("0"), json!(true)] {
            let mut malformed = valid.clone();
            malformed["root"] = root;
            assert!(graph(&malformed).is_none());
        }
    }

    #[test]
    fn preserves_record_sequences_and_object_only_struct_variants() {
        assert!(graph(&json!(["capture", 0, ["BytesZero"]])).is_some());
        assert!(graph(&json!(["capture", 0])).is_none());
        assert!(graph(&json!(["capture", 0, [], null])).is_none());
        assert!(source(&json!([0, "0x1"])).is_some());
        assert!(binding(&json!(["0x0", 0, null])).is_some());
        assert!(node(&json!({"SlotsInput": {"source": [0, "0x1"], "bindings": []}})).is_some());
        assert!(node(&json!({"SlotRead": {"state": 0, "key": 1}})).is_some());
        assert!(node(&json!({"SlotRead": [0, 1]})).is_none());
    }

    #[test]
    fn source_and_binding_keep_required_nullable_and_unknown_fields() {
        let input = source(&json!({"token": 0, "namespace": "0x1"})).unwrap();
        assert_eq!(input.namespace, U256::from(1));
        assert!(source(&json!({"token": 0})).is_none());
        assert!(source(&json!({"token": 0, "namespace": null})).is_none());
        assert!(source(&json!({"token": 0, "namespace": "0x1", "extra": 0})).is_none());
        let bound = binding(&json!({"index": "0x0", "value": 0, "numeric": null})).unwrap();
        assert!(bound.numeric.is_null());
        assert!(binding(&json!({"index": "0x0", "value": 0})).is_none());
        assert!(binding(&json!({"index": "0x0", "value": null, "numeric": null})).is_none());
        assert!(
            binding(&json!({"index": "0x0", "value": 0, "numeric": null, "extra": 0})).is_none()
        );
    }

    #[test]
    fn preserves_external_tags_unit_forms_and_checked_payloads() {
        for valid in [json!("BytesZero"), json!({"BytesZero": null})] {
            assert!(matches!(node(&valid), Some(Node::BytesZero)));
        }
        assert!(matches!(
            node(&json!({"BytesExact": [0, 255]})),
            Some(Node::BytesExact(_))
        ));
        for invalid in [
            json!(null),
            json!({}),
            json!("Constant"),
            json!("Unsupported"),
            json!({"Unsupported": null}),
            json!({"BytesZero": []}),
            json!({"BytesZero": {}}),
            json!({"BytesZero": false}),
            json!({"BytesZero": null, "Constant": "0x0"}),
            json!({"BytesExact": [256]}),
            json!({"BytesExact": [-1]}),
            json!({"BytesExact": [0.0]}),
            json!({"Constant": "0x10000000000000000000000000000000000000000000000000000000000000000"}),
            json!({"Slice": {"source": 0, "extra": null}}),
            json!({"Slice": {}}),
            json!({"Slice": {"source": null}}),
        ] {
            assert!(node(&invalid).is_none(), "{invalid}");
        }
    }
}
