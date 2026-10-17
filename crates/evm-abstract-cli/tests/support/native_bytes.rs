//! Read the concrete immutable byte roots used by the native issue13 fixtures.
//! Sparse cells are upper bounds; only the typed root can prove the full word.
//! Unsupported sources, guards and nonliteral operand facts fail closed.

use alloy_primitives::U256;
use evm_abstract::domain::{
    NumericValue,
    symbolic::{ByteStateId, ByteView, ExprId, ExprLimits},
};
use serde::Deserialize;
use serde_json::Value as Json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Graph {
    identity_scope: String,
    root: usize,
    nodes: Vec<Node>,
}

// This is deliberately the concrete subset observed in the complete captures,
// not a native-to-protocol converter or an evaluator for symbolic EVM words.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum Node {
    Constant(U256),
    BytesZero,
    BytesExact(Vec<u8>),
    WriteWord {
        previous: usize,
        offset: usize,
        value: usize,
    },
    View {
        state: usize,
        offset: usize,
        length: usize,
    },
    Slice {
        source: usize,
    },
    Copy {
        previous: usize,
        destination: usize,
        source: usize,
        length: usize,
    },
}

impl Node {
    fn references(&self) -> Vec<usize> {
        match *self {
            Self::Constant(_) | Self::BytesZero | Self::BytesExact(_) => vec![],
            Self::WriteWord {
                previous,
                offset,
                value,
            } => vec![previous, offset, value],
            Self::View {
                state,
                offset,
                length,
            } => vec![state, offset, length],
            Self::Slice { source } => vec![source],
            Self::Copy {
                previous,
                destination,
                source,
                length,
            } => vec![previous, destination, source, length],
        }
    }
}

enum Typed {
    Word(U256),
    State(ByteStateId),
    View(ByteView),
}

fn word(nodes: &[Typed], index: usize) -> Option<U256> {
    match nodes.get(index)? {
        Typed::Word(value) => Some(*value),
        _ => None,
    }
}

fn scalar(nodes: &[Typed], index: usize) -> Option<ExprId> {
    Some(ExprId::constant(word(nodes, index)?))
}

fn state(nodes: &[Typed], index: usize) -> Option<ByteStateId> {
    match nodes.get(index)? {
        Typed::State(state) => Some(state.clone()),
        _ => None,
    }
}

fn view(nodes: &[Typed], index: usize) -> Option<ByteView> {
    match nodes.get(index)? {
        Typed::View(view) => Some(view.clone()),
        _ => None,
    }
}

impl Graph {
    fn state(&self, limits: ExprLimits) -> Option<ByteStateId> {
        if self.identity_scope.is_empty()
            || self.nodes.len() > limits.max_nodes
            || self.root.checked_add(1)? != self.nodes.len()
        {
            return None;
        }
        let mut nodes = Vec::new();
        let mut sizes = Vec::<usize>::new();
        let mut depths = Vec::<usize>::new();
        let mut exact_bytes = 0usize;
        for (index, node) in self.nodes.iter().enumerate() {
            let refs = node.references();
            if refs.iter().any(|child| *child >= index) {
                return None;
            }
            let size = refs
                .iter()
                .try_fold(1usize, |size, child| size.checked_add(sizes[*child]))?;
            let depth = refs.iter().map(|child| depths[*child]).max().unwrap_or(0) + 1;
            if size > limits.max_nodes || depth > limits.max_depth {
                return None;
            }
            let typed = match node {
                Node::Constant(value) => Typed::Word(*value),
                Node::BytesZero => Typed::State(ByteStateId::zero()),
                Node::BytesExact(bytes) => {
                    exact_bytes = exact_bytes.checked_add(bytes.len())?;
                    if exact_bytes > limits.max_nodes {
                        return None;
                    }
                    Typed::State(ByteStateId::exact(bytes.clone().into()))
                }
                Node::WriteWord {
                    previous,
                    offset,
                    value,
                } => {
                    word(&nodes, *offset)?.checked_add(U256::from(32))?;
                    Typed::State(
                        state(&nodes, *previous)?
                            .write_word(
                                scalar(&nodes, *offset)?,
                                scalar(&nodes, *value)?,
                                limits.into(),
                            )
                            .ok()?,
                    )
                }
                Node::View {
                    state: source,
                    offset,
                    length,
                } => {
                    word(&nodes, *offset)?.checked_add(word(&nodes, *length)?)?;
                    Typed::View(ByteView::new(
                        state(&nodes, *source)?,
                        scalar(&nodes, *offset)?,
                        scalar(&nodes, *length)?,
                    ))
                }
                Node::Slice { source } => {
                    let source = view(&nodes, *source)?;
                    Typed::State(
                        source
                            .state
                            .slice(source.offset, source.length, limits.into())
                            .ok()?,
                    )
                }
                Node::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => {
                    word(&nodes, *destination)?.checked_add(word(&nodes, *length)?)?;
                    Typed::State(
                        state(&nodes, *previous)?
                            .copy(
                                scalar(&nodes, *destination)?,
                                view(&nodes, *source)?,
                                scalar(&nodes, *length)?,
                                limits.into(),
                            )
                            .ok()?,
                    )
                }
            };
            sizes.push(size);
            depths.push(depth);
            nodes.push(typed);
        }
        // Native export contains only reachable nodes. Reject orphan subtrees,
        // including an unrelated literal that could otherwise justify a fact.
        let mut reached = vec![false; self.nodes.len()];
        *reached.get_mut(self.root)? = true;
        for index in (0..self.nodes.len()).rev() {
            if reached[index] {
                for child in self.nodes[index].references() {
                    reached[child] = true;
                }
            }
        }
        reached.iter().all(|reached| *reached).then_some(())?;
        state(&nodes, self.root)
    }
}

fn literal(value: &Json, scope: &str) -> Option<U256> {
    let graph: Graph = serde_json::from_value(value.clone()).ok()?;
    if graph.identity_scope != scope || graph.root != 0 {
        return None;
    }
    match graph.nodes.as_slice() {
        [Node::Constant(value)] => Some(*value),
        _ => None,
    }
}

// Compare complete numeric fields against public typed serialization. We only
// need exact literals and the captured unknown-byte presentation. Provenance is
// metadata, not a numeric proof, and remains in the untouched native relation.
fn numeric_matches(value: &Json, expected: NumericValue, scope: &str) -> Option<()> {
    let mut numeric = value.as_object()?.clone();
    numeric.remove("provenance")?.as_object()?;
    if let Some(expression) = numeric.remove("expression") {
        (literal(&expression, scope)? == expected.singleton()?).then_some(())?;
    }
    (Json::Object(numeric) == serde_json::to_value(expected).ok()?).then_some(())
}

fn read_word(data: &Json) -> Option<[u8; 32]> {
    let graph: Graph = serde_json::from_value(data.get("root")?.clone()).ok()?;
    let scope = &graph.identity_scope;
    if !data.get("failure")?.is_null()
        || data.get("memory")?.as_bool()?
        || !data.get("conditional")?.as_array()?.is_empty()
    {
        return None;
    }
    numeric_matches(
        data.get("length")?,
        NumericValue::constant(U256::from(32)),
        scope,
    )?;
    let limits = ExprLimits::default();
    let root = graph.state(limits)?;
    for fact in data.get("operands")?.as_array()? {
        let fields = fact.as_object()?;
        if fields.len() != 2 {
            return None;
        }
        let value = literal(fields.get("expression")?, scope)?;
        graph
            .nodes
            .iter()
            .any(|node| matches!(node, Node::Constant(word) if *word == value))
            .then_some(())?;
        numeric_matches(fields.get("value")?, NumericValue::constant(value), scope)?;
    }
    let bytes: [u8; 32] = ByteView::new(
        root,
        ExprId::constant(U256::ZERO),
        ExprId::constant(U256::from(32)),
    )
    .known_bytes(limits)
    .ok()??
    .try_into()
    .ok()?;
    let cells = data.get("bytes")?.as_object()?;
    let default = data.get("default")?;
    for offset in cells.keys() {
        let index = offset.parse::<usize>().ok()?;
        if index >= bytes.len() || index.to_string() != *offset {
            return None;
        }
    }
    for (offset, byte) in bytes.iter().enumerate() {
        let bound = cells.get(&offset.to_string()).unwrap_or(default);
        numeric_matches(bound, NumericValue::constant(U256::from(*byte)), scope)
            .or_else(|| numeric_matches(bound, NumericValue::unknown_byte(), scope))?;
    }
    Some(bytes)
}

pub fn word_matches(data: &Json, expected: U256) -> bool {
    read_word(data) == Some(expected.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::{ByteStateId, ByteView, ExprId, ExprLimits, Json, U256, word_matches};
    use evm_abstract::{domain::AbstractValue, world::ByteArray};
    use serde_json::json;

    fn snapshot(root: ByteStateId) -> Json {
        let mut data = serde_json::to_value(ByteArray::exact(&[0; 32])).unwrap();
        data["root"] = serde_json::to_value(root).unwrap();
        data["bytes"] = json!({});
        data["default"] = serde_json::to_value(AbstractValue::unknown_byte()).unwrap();
        data
    }

    fn written(value: u64) -> ByteStateId {
        ByteStateId::zero()
            .write_word(
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(value)),
                ExprLimits::default().into(),
            )
            .unwrap()
    }

    #[test]
    fn requires_the_complete_word_and_exact_length() {
        let expected = U256::from(42);
        let data = snapshot(written(42));
        assert!(word_matches(&data, expected));
        assert!(!word_matches(&data, U256::from(7)));
        let mut prefix = expected.to_be_bytes::<32>();
        prefix[0] = 1;
        assert!(!word_matches(
            &snapshot(ByteStateId::exact(prefix.to_vec().into())),
            expected
        ));
        for length in [31usize, 33] {
            let mut data = data.clone();
            data["length"] =
                serde_json::to_value(AbstractValue::constant(U256::from(length))).unwrap();
            assert!(!word_matches(&data, expected));
        }
    }

    #[test]
    fn preserves_copy_source_versions_padding_and_later_writes() {
        let expected = U256::from(42);
        let source = written(42);
        let copy = |source: ByteStateId, length: usize| {
            written(7)
                .copy(
                    ExprId::constant(U256::ZERO),
                    ByteView::new(
                        source,
                        ExprId::constant(U256::ZERO),
                        ExprId::constant(U256::from(length)),
                    ),
                    ExprId::constant(U256::from(32)),
                    ExprLimits::default().into(),
                )
                .unwrap()
        };
        let captured = copy(source.clone(), 32);
        let changed_source = source
            .write_word(
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(7)),
                ExprLimits::default().into(),
            )
            .unwrap();
        assert!(word_matches(&snapshot(captured.clone()), expected));
        assert!(!word_matches(&snapshot(copy(changed_source, 32)), expected));
        assert!(!word_matches(&snapshot(copy(source, 31)), expected));
        let overwritten = captured
            .write_word(
                ExprId::constant(U256::from(31)),
                ExprId::constant(U256::from(7)),
                ExprLimits::default().into(),
            )
            .unwrap();
        assert!(!word_matches(&snapshot(overwritten), expected));
    }

    #[test]
    fn requires_same_scope_literal_facts_and_supported_guards() {
        let expected = U256::from(42);
        let mut data = snapshot(written(42));
        data["operands"] = json!([{
            "expression": ExprId::constant(expected),
            "value": AbstractValue::constant(expected)
        }]);
        assert!(word_matches(&data, expected));
        let mut wrong_fact = data.clone();
        wrong_fact["operands"][0]["value"] =
            serde_json::to_value(AbstractValue::constant(U256::from(7))).unwrap();
        assert!(!word_matches(&wrong_fact, expected));
        data["operands"][0]["expression"]["identity_scope"] = json!("another-producer");
        assert!(!word_matches(&data, expected));
        let unknown = written(42)
            .join_guarded(
                &written(7),
                ExprId::fresh().unwrap(),
                ExprLimits::default().into(),
            )
            .unwrap();
        assert!(!word_matches(&snapshot(unknown), expected));
    }

    #[test]
    fn rejects_missing_failed_and_malformed_snapshots() {
        let expected = U256::from(42);
        let data = snapshot(written(42));
        let mut failed = data.clone();
        failed["failure"] = json!("captured failure");
        assert!(!word_matches(&failed, expected));
        let mut missing = data.clone();
        missing.as_object_mut().unwrap().remove("operands");
        assert!(!word_matches(&missing, expected));
        let mut forward = data.clone();
        forward["root"]["nodes"][3]["WriteWord"]["previous"] = json!(3);
        assert!(!word_matches(&forward, expected));
        let mut wrong_sort = data.clone();
        wrong_sort["root"]["nodes"][3]["WriteWord"]["value"] = json!(0);
        assert!(!word_matches(&wrong_sort, expected));
        let mut truncated = data;
        truncated["root"]["nodes"].as_array_mut().unwrap().pop();
        assert!(!word_matches(&truncated, expected));
    }
}
