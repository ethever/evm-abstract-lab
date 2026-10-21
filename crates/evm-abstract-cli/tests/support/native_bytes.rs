//! Read the concrete immutable byte roots used by the native issue13 fixtures.
//! Sparse cells are upper bounds; only the typed root can prove the full word.
//! Only explicit snapshot bindings with matching read facts extend literals.
//! Unsupported sources, guards and all other nonliteral facts fail closed.

use alloy_primitives::U256;
use evm_abstract::domain::{
    NumericValue, WordObservation,
    symbolic::{
        ByteStateId, ByteView, ExprId, ExprLimits, SlotStateId, SlotStateKind, SourceId,
        StateBinding,
    },
};
use serde_json::Value as Json;
use std::collections::BTreeSet;

#[path = "native_bytes/decode.rs"]
mod decode;

struct Graph {
    identity_scope: String,
    root: usize,
    nodes: Vec<Node>,
}

struct Source {
    token: usize,
    namespace: U256,
}

struct Binding {
    index: U256,
    value: usize,
    numeric: Json,
}

// This is deliberately the concrete subset observed in the complete captures,
// not a native-to-protocol converter or an evaluator for symbolic EVM words.
enum Node {
    Constant(U256),
    SlotsInput {
        source: Source,
        bindings: Vec<Binding>,
    },
    SlotRead {
        state: usize,
        key: usize,
    },
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
            Self::SlotsInput {
                ref source,
                ref bindings,
            } => std::iter::once(source.token)
                .chain(bindings.iter().map(|binding| binding.value))
                .collect(),
            Self::SlotRead { state, key } => vec![state, key],
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

// The observed value never replaces the original read during identity checks.
// It is usable by the byte copy only after every operand certificate passes.
#[derive(Clone, PartialEq, Eq)]
struct Scalar {
    original: ExprId,
    exact: U256,
}

enum Typed {
    Word(Scalar),
    Slots(SlotStateId),
    State(ByteStateId),
    View(ByteView),
}

fn word(nodes: &[Typed], index: usize) -> Option<U256> {
    scalar(nodes, index)?.as_constant()
}

fn typed_scalar(nodes: &[Typed], index: usize) -> Option<&Scalar> {
    match nodes.get(index)? {
        Typed::Word(value) => Some(value),
        _ => None,
    }
}

fn scalar(nodes: &[Typed], index: usize) -> Option<ExprId> {
    Some(typed_scalar(nodes, index)?.original.clone())
}

fn slots(nodes: &[Typed], index: usize) -> Option<SlotStateId> {
    match nodes.get(index)? {
        Typed::Slots(state) => Some(state.clone()),
        _ => None,
    }
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
    fn typed(&self, limits: ExprLimits, certified: bool) -> Option<Vec<Typed>> {
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
                Node::Constant(value) => Typed::Word(Scalar {
                    original: ExprId::constant(*value),
                    exact: *value,
                }),
                Node::SlotsInput { source, bindings } => {
                    // Tokens and binding values must be earlier literal scalars;
                    // a read's exact value cannot stand in for either identity.
                    word(&nodes, source.token)?;
                    if bindings.is_empty() {
                        return None;
                    }
                    let mut indices = BTreeSet::new();
                    let mut observed = Vec::new();
                    for binding in bindings {
                        if !indices.insert(binding.index) {
                            return None;
                        }
                        let numeric = WordObservation::constant(word(&nodes, binding.value)?);
                        if binding.numeric != serde_json::to_value(&numeric).ok()? {
                            return None;
                        }
                        observed.push(StateBinding {
                            index: binding.index,
                            value: scalar(&nodes, binding.value)?,
                            numeric,
                        });
                    }
                    Typed::Slots(
                        SlotStateId::observed(
                            SourceId::scoped(scalar(&nodes, source.token)?, source.namespace),
                            observed,
                            limits.into(),
                        )
                        .ok()?,
                    )
                }
                Node::SlotRead { state, key } => {
                    let state = slots(&nodes, *state)?;
                    let key_value = word(&nodes, *key)?;
                    let SlotStateKind::Input { bindings, .. } = state.kind() else {
                        return None;
                    };
                    let binding = bindings.iter().find(|binding| binding.index == key_value)?;
                    let exact = binding.value.as_constant()?;
                    (binding.numeric.singleton()? == exact).then_some(())?;
                    Typed::Word(Scalar {
                        original: ExprId::slot_read(state, scalar(&nodes, *key)?, limits).ok()?,
                        exact,
                    })
                }
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
                    let value = typed_scalar(&nodes, *value)?;
                    Typed::State(
                        state(&nodes, *previous)?
                            .write_word(
                                scalar(&nodes, *offset)?,
                                if certified {
                                    ExprId::constant(value.exact)
                                } else {
                                    value.original.clone()
                                },
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
        Some(nodes)
    }
}

fn read_scalar(value: &Json, scope: &str, limits: ExprLimits) -> Option<Scalar> {
    let graph = decode::graph(value)?;
    (graph.identity_scope == scope).then_some(())?;
    let nodes = graph.typed(limits, false)?;
    Some(typed_scalar(&nodes, graph.root)?.clone())
}

fn literal(value: &Json, scope: &str) -> Option<U256> {
    let graph = decode::graph(value)?;
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

fn read_fact_matches(
    value: &Json,
    expected: &Scalar,
    scope: &str,
    limits: ExprLimits,
) -> Option<()> {
    let mut numeric = value.as_object()?.clone();
    numeric.remove("provenance")?.as_object()?;
    // Unlike literal presentation, a read requires its complete original graph
    // inside the numeric fact too. Equal scalar values do not identify sources.
    let expression = numeric.remove("expression")?;
    (read_scalar(&expression, scope, limits)? == *expected).then_some(())?;
    (Json::Object(numeric) == serde_json::to_value(NumericValue::constant(expected.exact)).ok()?)
        .then_some(())
}

fn read_word(data: &Json) -> Option<[u8; 32]> {
    let graph = decode::graph(data.get("root")?)?;
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
    let nodes = graph.typed(limits, false)?;
    state(&nodes, graph.root)?;
    let mut certified_reads = Vec::new();
    for fact in data.get("operands")?.as_array()? {
        let fields = fact.as_object()?;
        if fields.len() != 2 {
            return None;
        }
        let value = read_scalar(fields.get("expression")?, scope, limits)?;
        nodes
            .iter()
            .any(|node| matches!(node, Typed::Word(word) if *word == value))
            .then_some(())?;
        if value.original.as_constant().is_some() {
            numeric_matches(
                fields.get("value")?,
                NumericValue::constant(value.exact),
                scope,
            )?;
        } else {
            read_fact_matches(fields.get("value")?, &value, scope, limits)?;
            certified_reads.push(value.original);
        }
    }
    for node in &nodes {
        if let Typed::Word(value) = node
            && value.original.as_constant().is_none()
            && !certified_reads.contains(&value.original)
        {
            return None;
        }
    }
    // All identities, bindings and operand facts have passed. Only this private
    // byte-materialization copy substitutes literals for the certified reads.
    let materialized = graph.typed(limits, true)?;
    let root = state(&materialized, graph.root)?;
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
    use super::{
        ByteStateId, ByteView, ExprId, ExprLimits, Json, SlotStateId, SourceId, StateBinding, U256,
        WordObservation, word_matches,
    };
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

    fn slot_snapshot(token: u64, namespace: u64, value: u64) -> Json {
        let limits = ExprLimits::default();
        let value = U256::from(value);
        let read = ExprId::slot_read(
            SlotStateId::observed(
                SourceId::scoped(ExprId::constant(U256::from(token)), U256::from(namespace)),
                vec![StateBinding {
                    index: U256::ZERO,
                    value: ExprId::constant(value),
                    numeric: WordObservation::constant(value),
                }],
                limits.into(),
            )
            .unwrap(),
            ExprId::constant(U256::ZERO),
            limits,
        )
        .unwrap();
        assert!(read.as_constant().is_none());
        let root = ByteStateId::zero()
            .write_word(ExprId::constant(U256::ZERO), read.clone(), limits.into())
            .unwrap()
            .slice(
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(32)),
                limits.into(),
            )
            .unwrap();
        let mut data = snapshot(root);
        let mut numeric = serde_json::to_value(AbstractValue::constant(value)).unwrap();
        numeric["expression"] = serde_json::to_value(&read).unwrap();
        data["operands"] = json!([{"expression": read, "value": numeric}]);
        data
    }

    fn input_binding(data: &mut Json) -> &mut Json {
        data["root"]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find_map(|node| node.get_mut("SlotsInput"))
            .unwrap()
    }

    #[test]
    fn requires_explicit_exact_binding_and_complete_read_fact() {
        let expected = U256::from(42);
        let data = slot_snapshot(100, 0x10100, 42);
        assert!(word_matches(&data, expected));
        assert!(!word_matches(&data, U256::from(7)));
        let mut missing_fact = data.clone();
        missing_fact["operands"] = json!([]);
        assert!(!word_matches(&missing_fact, expected));
        let mut missing_expression = data.clone();
        missing_expression["operands"][0]["value"]
            .as_object_mut()
            .unwrap()
            .remove("expression");
        assert!(!word_matches(&missing_expression, expected));
        let mut incomplete_fact = data.clone();
        incomplete_fact["operands"][0]["value"]
            .as_object_mut()
            .unwrap()
            .remove("known_bits");
        assert!(!word_matches(&incomplete_fact, expected));
        let mut missing_constraint = data.clone();
        input_binding(&mut missing_constraint)["bindings"][0]["numeric"]
            .as_object_mut()
            .unwrap()
            .remove("Constants");
        assert!(!word_matches(&missing_constraint, expected));
        let mut conflicting_constraint = data.clone();
        input_binding(&mut conflicting_constraint)["bindings"][0]["numeric"]["Constants"] =
            json!(["0x7"]);
        assert!(!word_matches(&conflicting_constraint, expected));
        let mut duplicate = data.clone();
        let bindings = input_binding(&mut duplicate)["bindings"]
            .as_array_mut()
            .unwrap();
        bindings.push(bindings[0].clone());
        assert!(!word_matches(&duplicate, expected));
        let mut unobserved = data.clone();
        input_binding(&mut unobserved)["bindings"][0]["index"] = json!("0x1");
        assert!(!word_matches(&unobserved, expected));
        let mut forward_token = data.clone();
        input_binding(&mut forward_token)["source"]["token"] = json!(usize::MAX);
        assert!(!word_matches(&forward_token, expected));
        let mut wrong_sort = data.clone();
        input_binding(&mut wrong_sort)["bindings"][0]["value"] = json!(0);
        assert!(!word_matches(&wrong_sort, expected));
        let mut missing_binding = data;
        input_binding(&mut missing_binding)["bindings"] = json!([]);
        assert!(!word_matches(&missing_binding, expected));
    }

    #[test]
    fn requires_original_token_namespace_key_binding_and_producer_scope() {
        let expected = U256::from(42);
        let data = slot_snapshot(100, 0x10100, 42);
        // Each replacement has coherent own evidence. Equal scalar values
        // cannot hide a different token, namespace, binding, or read key.
        for other in [
            slot_snapshot(101, 0x10100, 42),
            slot_snapshot(100, 0x10200, 42),
            slot_snapshot(100, 0x10100, 7),
        ] {
            let mut wrong_source = data.clone();
            wrong_source["operands"] = other["operands"].clone();
            assert!(!word_matches(&wrong_source, expected));
        }
        let mut wrong_key = data.clone();
        for node in wrong_key["operands"][0]["expression"]["nodes"]
            .as_array_mut()
            .unwrap()
        {
            if node["Constant"] == "0x0" {
                node["Constant"] = json!("0x1");
            }
            if let Some(input) = node.get_mut("SlotsInput") {
                input["bindings"][0]["index"] = json!("0x1");
            }
        }
        wrong_key["operands"][0]["value"]["expression"] =
            wrong_key["operands"][0]["expression"].clone();
        assert!(!word_matches(&wrong_key, expected));
        let mut wrong_scope = data.clone();
        wrong_scope["operands"][0]["expression"]["identity_scope"] = json!("other-producer");
        assert!(!word_matches(&wrong_scope, expected));
        let mut wrong_numeric_scope = data.clone();
        wrong_numeric_scope["operands"][0]["value"]["expression"]["identity_scope"] =
            json!("other-producer");
        assert!(!word_matches(&wrong_numeric_scope, expected));
        let mut literal_fact = data;
        literal_fact["operands"] = json!([{
            "expression": ExprId::constant(expected),
            "value": AbstractValue::constant(expected)
        }]);
        assert!(!word_matches(&literal_fact, expected));
    }

    #[test]
    fn owners_with_the_same_key_cannot_borrow_each_others_evidence() {
        let owner42 = slot_snapshot(100, 0x10100, 42);
        let owner7 = slot_snapshot(100, 0x20000, 7);
        assert!(word_matches(&owner42, U256::from(42)));
        assert!(word_matches(&owner7, U256::from(7)));
        assert!(!word_matches(&owner7, U256::from(42)));
        let mut borrowed42 = owner42.clone();
        borrowed42["operands"] = owner7["operands"].clone();
        assert!(!word_matches(&borrowed42, U256::from(42)));
        assert!(!word_matches(&borrowed42, U256::from(7)));
        let mut borrowed7 = owner7;
        borrowed7["operands"] = owner42["operands"].clone();
        assert!(!word_matches(&borrowed7, U256::from(7)));
        assert!(!word_matches(&borrowed7, U256::from(42)));
    }
}
