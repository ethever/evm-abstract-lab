//! Whole-array differential checks, independent of the constructor reductions.

use super::super::super::{SlotStateKind, StateBinding};
use super::{
    ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind, GraphLimits, SlotStateId, SourceId,
    StateError, StateLimits, SymbolicGraph, SymbolicGraphNode, c, limits,
};
use crate::domain::NumericValue;
use alloy_primitives::U256;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const GUARD_VALUES: [U256; 4] = [
    U256::ZERO,
    U256::ONE,
    U256::from_limbs([2, 0, 0, 0]),
    U256::MAX,
];

/// Finite concrete sources with zero elsewhere give complete total arrays.
/// Byte vectors discard trailing zeroes; slot maps discard zero-valued cells.
/// This interpreter follows original array semantics, without using symbolic
/// reads, join constructors, fingerprints, or the five algebraic reductions.
#[derive(Default)]
struct Valuation {
    words: BTreeMap<ExprId, U256>,
    bytes: BTreeMap<SourceId, Vec<u8>>,
    slots: BTreeMap<SourceId, BTreeMap<U256, U256>>,
}

impl Valuation {
    fn word(&self, expression: &ExprId) -> U256 {
        match expression.kind() {
            ExprKind::Constant(value) => *value,
            ExprKind::Fresh(_) => self.words[expression],
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => self.word(if self.word(condition) != U256::ZERO {
                then_value
            } else {
                else_value
            }),
            _ => panic!("unexpected scalar in concrete state fixture"),
        }
    }

    fn index(&self, expression: &ExprId) -> usize {
        usize::try_from(self.word(expression)).expect("finite byte fixture")
    }

    fn view(&self, view: &ByteView) -> Vec<u8> {
        let bytes = self.byte_array(&view.state);
        let offset = self.index(&view.offset);
        (0..self.index(&view.length))
            .map(|i| bytes.get(offset + i).copied().unwrap_or(0))
            .collect()
    }

    fn byte_array(&self, root: &ByteStateId) -> Vec<u8> {
        let mut bytes = match root.kind() {
            ByteStateKind::Zero => Vec::new(),
            ByteStateKind::Exact(bytes) => bytes.to_vec(),
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => {
                let mut bytes = self.bytes[source].clone();
                bytes.resize(self.index(length), 0);
                for binding in bindings {
                    let at = usize::try_from(binding.index).unwrap();
                    if let Some(byte) = bytes.get_mut(at) {
                        *byte = self.word(&binding.value).to_le_bytes::<32>()[0];
                    }
                }
                bytes
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            }
            | ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let mut bytes = self.byte_array(previous);
                let at = self.index(offset);
                let value = self.word(value);
                let written = if matches!(root.kind(), ByteStateKind::WriteByte { .. }) {
                    vec![value.to_le_bytes::<32>()[0]]
                } else {
                    value.to_be_bytes::<32>().to_vec()
                };
                bytes.resize(bytes.len().max(at + written.len()), 0);
                bytes[at..at + written.len()].copy_from_slice(&written);
                bytes
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let mut bytes = self.byte_array(previous);
                let mut copied = self.view(source);
                copied.resize(self.index(length), 0);
                let at = self.index(destination);
                bytes.resize(bytes.len().max(at + copied.len()), 0);
                bytes[at..at + copied.len()].copy_from_slice(&copied);
                bytes
            }
            ByteStateKind::Slice { source } => self.view(source),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => self.byte_array(if self.word(condition) != U256::ZERO {
                then_state
            } else {
                else_state
            }),
        };
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        bytes
    }

    fn slot_array(&self, root: &SlotStateId) -> BTreeMap<U256, U256> {
        let mut slots = match root.kind() {
            SlotStateKind::Zero => BTreeMap::new(),
            SlotStateKind::Input { source, bindings } => {
                let mut slots = self.slots[source].clone();
                for binding in bindings {
                    slots.insert(binding.index, self.word(&binding.value));
                }
                slots
            }
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => {
                let mut slots = self.slot_array(previous);
                slots.insert(self.word(key), self.word(value));
                slots
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => self.slot_array(if self.word(condition) != U256::ZERO {
                then_state
            } else {
                else_state
            }),
        };
        slots.retain(|_, value| *value != U256::ZERO);
        slots
    }

    fn assign(&mut self, guards: &[ExprId], mut index: usize) {
        for guard in guards {
            self.words
                .insert(guard.clone(), GUARD_VALUES[index % GUARD_VALUES.len()]);
            index /= GUARD_VALUES.len();
        }
    }
}

macro_rules! five_joins {
    ($a:expr, $b:expr, $guards:expr, $limits:expr) => {{
        let [s, t, h] = $guards;
        let left = $a.join_guarded($b, t.clone(), $limits).unwrap();
        let right = $a.join_guarded($b, h.clone(), $limits).unwrap();
        [
            left.join_guarded($a, s.clone(), $limits).unwrap(),
            left.join_guarded($b, s.clone(), $limits).unwrap(),
            $a.join_guarded(&left, s.clone(), $limits).unwrap(),
            $b.join_guarded(&left, s.clone(), $limits).unwrap(),
            left.join_guarded(&right, s.clone(), $limits).unwrap(),
        ]
    }};
}

fn byte_pair() -> (ByteStateId, ByteStateId) {
    let make = |values: [u64; 3]| {
        ByteStateId::zero()
            .write_byte(c(0), c(values[0]), limits())
            .unwrap()
            .write_byte(c(7), c(values[1]), limits())
            .unwrap()
            .write_byte(c(35), c(values[2]), limits())
            .unwrap()
    };
    (make([7, 0x81, 0x52]), make([7, 0x24, 0xf3]))
}

fn slot_pair() -> (SlotStateId, SlotStateId) {
    let make = |middle, last| {
        SlotStateId::zero()
            .store(c(0), c(7), limits())
            .unwrap()
            .store(c(37), c(middle), limits())
            .unwrap()
            .store(ExprId::constant(U256::MAX), c(last), limits())
            .unwrap()
    };
    (make(0x81, 0x52), make(0x24, 0xf3))
}

#[test]
fn all_five_shared_branch_rules_preserve_complete_arrays_for_independent_word_guards() {
    let guards = std::array::from_fn::<_, 3, _>(|_| ExprId::fresh().unwrap());
    let (a, b) = byte_pair();
    let (sa, sb) = slot_pair();
    let bytes = five_joins!(&a, &b, &guards, limits());
    let slots = five_joins!(&sa, &sb, &guards, limits());
    // Opposite branch order is outside the authorized fifth rule.
    let opposite_bytes = a
        .join_guarded(&b, guards[1].clone(), limits())
        .unwrap()
        .join_guarded(
            &b.join_guarded(&a, guards[2].clone(), limits()).unwrap(),
            guards[0].clone(),
            limits(),
        )
        .unwrap();
    let opposite_slots = sa
        .join_guarded(&sb, guards[1].clone(), limits())
        .unwrap()
        .join_guarded(
            &sb.join_guarded(&sa, guards[2].clone(), limits()).unwrap(),
            guards[0].clone(),
            limits(),
        )
        .unwrap();
    let mut oracle = Valuation::default();
    let mut expected_a = vec![0; 36];
    let mut expected_b = expected_a.clone();
    expected_a[0] = 7;
    expected_a[7] = 0x81;
    expected_a[35] = 0x52;
    expected_b[0] = 7;
    expected_b[7] = 0x24;
    expected_b[35] = 0xf3;
    let slot_a = BTreeMap::from([
        (U256::ZERO, U256::from(7)),
        (U256::from(37), U256::from(0x81)),
        (U256::MAX, U256::from(0x52)),
    ]);
    let slot_b = BTreeMap::from([
        (U256::ZERO, U256::from(7)),
        (U256::from(37), U256::from(0x24)),
        (U256::MAX, U256::from(0xf3)),
    ]);
    assert_eq!(oracle.byte_array(&a), expected_a);
    assert_eq!(oracle.byte_array(&b), expected_b);
    assert_eq!(oracle.slot_array(&sa), slot_a);
    assert_eq!(oracle.slot_array(&sb), slot_b);
    // Full structural equality still canonicalizes genuinely identical states.
    assert_eq!(byte_pair(), (a.clone(), b.clone()));
    assert_eq!(slot_pair(), (sa.clone(), sb.clone()));
    for assignment in 0..4usize.pow(3) {
        oracle.assign(&guards, assignment);
        let [s, t, h] = guards.each_ref().map(|g| oracle.word(g) != U256::ZERO);
        let take_a = [!s || t, s && t, s || t, !s && t, if s { t } else { h }];
        for rule in 0..5 {
            assert_eq!(
                oracle.byte_array(&bytes[rule]),
                *if take_a[rule] {
                    &expected_a
                } else {
                    &expected_b
                },
                "byte rule {rule}, assignment {assignment}"
            );
            assert_eq!(
                oracle.slot_array(&slots[rule]),
                *if take_a[rule] { &slot_a } else { &slot_b },
                "slot rule {rule}, assignment {assignment}"
            );
        }
        let opposite_a = if s { t } else { !h };
        assert_eq!(
            oracle.byte_array(&opposite_bytes),
            *if opposite_a { &expected_a } else { &expected_b }
        );
        assert_eq!(
            oracle.slot_array(&opposite_slots),
            *if opposite_a { &slot_a } else { &slot_b }
        );
    }
    for (rule, (bytes, slots)) in bytes.iter().zip(&slots).enumerate() {
        let expected: BTreeSet<_> = guards[..if rule == 4 { 3 } else { 2 }]
            .iter()
            .cloned()
            .collect();
        assert_eq!(bytes.fresh_leaves(), expected);
        assert_eq!(slots.fresh_leaves(), expected);
    }
}

macro_rules! negative_joins {
    ($a:expr, $changed:expr, $b:expr, $guards:expr) => {{
        let [s, t, h] = $guards;
        let left = $a.join_guarded($b, t.clone(), limits()).unwrap();
        let reversed = $b.join_guarded($a, t.clone(), limits()).unwrap();
        let different = $changed.join_guarded($b, h.clone(), limits()).unwrap();
        [
            left.join_guarded($changed, s.clone(), limits()).unwrap(),
            reversed
                .join_guarded($changed, s.clone(), limits())
                .unwrap(),
            $changed.join_guarded(&left, s.clone(), limits()).unwrap(),
            $changed
                .join_guarded(&reversed, s.clone(), limits())
                .unwrap(),
            left.join_guarded(&different, s.clone(), limits()).unwrap(),
        ]
    }};
}

/// Original syntax trees select full concrete arrays, without deriving a guard.
fn negative_expected<T: Clone>(a: &T, changed: &T, b: &T, [s, t, h]: [bool; 3]) -> [T; 5] {
    [
        if s { if t { a } else { b } } else { changed }.clone(),
        if s { if t { b } else { a } } else { changed }.clone(),
        if s {
            changed
        } else if t {
            a
        } else {
            b
        }
        .clone(),
        if s {
            changed
        } else if t {
            b
        } else {
            a
        }
        .clone(),
        if s {
            if t { a } else { b }
        } else if h {
            changed
        } else {
            b
        }
        .clone(),
    ]
}

#[test]
fn shared_observations_do_not_merge_source_namespaces_snapshots_old_writes_or_copies() {
    let guards = std::array::from_fn::<_, 3, _>(|_| ExprId::fresh().unwrap());
    let token = ExprId::fresh().unwrap();
    let sources = [
        SourceId::scoped(token.clone(), U256::ONE),
        SourceId::scoped(token, U256::from(2)),
        SourceId::fresh().unwrap(),
    ];
    let mut oracle = Valuation::default();
    let mut byte_sources = Vec::new();
    let mut slot_sources = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let binding = StateBinding {
            index: U256::ZERO,
            value: c(7),
            numeric: NumericValue::constant(U256::from(7)),
        };
        oracle
            .bytes
            .insert(source.clone(), vec![7, 11, 20 + i as u8]);
        oracle.slots.insert(
            source.clone(),
            BTreeMap::from([
                (U256::ZERO, U256::from(7)),
                (U256::from(37), U256::from(20 + i)),
            ]),
        );
        byte_sources.push(
            ByteStateId::observed(source.clone(), c(3), vec![binding.clone()], limits()).unwrap(),
        );
        slot_sources.push(SlotStateId::observed(source.clone(), vec![binding], limits()).unwrap());
    }
    let old = byte_sources[0].write_byte(c(2), c(0xa1), limits()).unwrap();
    let new = old.write_byte(c(2), c(0xa2), limits()).unwrap();
    let old_view = ByteView::new(old.clone(), c(0), c(3));
    let new_view = ByteView::new(new.clone(), c(0), c(3));
    assert_ne!(old_view, new_view);
    let old_copy = ByteStateId::zero()
        .copy(c(4), old_view.clone(), c(3), limits())
        .unwrap();
    let new_copy = ByteStateId::zero()
        .copy(c(4), new_view, c(3), limits())
        .unwrap();
    assert_eq!(oracle.byte_array(&old_copy)[4], 7);
    assert_eq!(oracle.byte_array(&new_copy)[4], 7);
    let old_slot = slot_sources[0].store(c(37), c(0xa1), limits()).unwrap();
    let new_slot = old_slot.store(c(37), c(0xa2), limits()).unwrap();
    let byte_negatives = [
        (
            ByteStateId::exact(Arc::from([7, 11, 20])),
            ByteStateId::exact(Arc::from([7, 11, 21])),
        ),
        (byte_sources[0].clone(), byte_sources[1].clone()),
        (byte_sources[0].clone(), byte_sources[2].clone()),
        (old.clone(), new.clone()),
        (old_copy, new_copy),
    ];
    for (a, changed) in byte_negatives {
        assert_ne!(a, changed);
        let before = (a.kind().clone(), changed.kind().clone());
        let original = oracle.byte_array(&a);
        let different = oracle.byte_array(&changed);
        assert_eq!(original[0], different[0]);
        assert_ne!(original, different);
        let b = ByteStateId::exact(Arc::from([0x31, 0x55, 0x91]));
        let roots = negative_joins!(&a, &changed, &b, &guards);
        for assignment in 0..4usize.pow(3) {
            oracle.assign(&guards, assignment);
            let values = guards.each_ref().map(|g| oracle.word(g) != U256::ZERO);
            let expected =
                negative_expected(&original, &different, &vec![0x31, 0x55, 0x91], values);
            for (root, expected) in roots.iter().zip(expected) {
                assert_eq!(oracle.byte_array(root), expected);
            }
        }
        assert_eq!((a.kind().clone(), changed.kind().clone()), before);
    }
    let (concrete_a, _) = slot_pair();
    let concrete_changed = concrete_a.store(c(37), c(0x82), limits()).unwrap();
    for (a, changed) in [
        (concrete_a, concrete_changed),
        (slot_sources[0].clone(), slot_sources[1].clone()),
        (slot_sources[0].clone(), slot_sources[2].clone()),
        (old_slot, new_slot),
    ] {
        assert_ne!(a, changed);
        let before = (a.kind().clone(), changed.kind().clone());
        let original = oracle.slot_array(&a);
        let different = oracle.slot_array(&changed);
        assert_eq!(original[&U256::ZERO], different[&U256::ZERO]);
        assert_ne!(original, different);
        let b = SlotStateId::zero().store(c(9), c(0x55), limits()).unwrap();
        let roots = negative_joins!(&a, &changed, &b, &guards);
        for assignment in 0..4usize.pow(3) {
            oracle.assign(&guards, assignment);
            let values = guards.each_ref().map(|g| oracle.word(g) != U256::ZERO);
            let expected = negative_expected(
                &original,
                &different,
                &BTreeMap::from([(U256::from(9), U256::from(0x55))]),
                values,
            );
            for (root, expected) in roots.iter().zip(expected) {
                assert_eq!(oracle.slot_array(root), expected);
            }
        }
        assert_eq!((a.kind().clone(), changed.kind().clone()), before);
    }
    assert_eq!(oracle.view(&old_view), vec![7, 11, 0xa1]);
    assert_eq!(oracle.byte_array(&old), vec![7, 11, 0xa1]);
    assert_eq!(oracle.byte_array(&new), vec![7, 11, 0xa2]);
}

macro_rules! repeated_roots {
    ($a:expr, $b:expr, $g:expr, $limits:expr) => {{
        let [g, h, k, n, m] = $g;
        let first = $a.join_guarded($b, k.clone(), $limits).unwrap();
        let middle = $a.join_guarded(&first, h.clone(), $limits).unwrap();
        let repeated = $a.join_guarded(&middle, g.clone(), $limits).unwrap();
        let parallel = $a.join_guarded($b, m.clone(), $limits).unwrap();
        let diamond = parallel.join_guarded(&first, n.clone(), $limits).unwrap();
        (repeated, diamond)
    }};
}

#[test]
fn probe40_repeated_complete_roots_fit_tight_limits_with_smaller_retained_state_dags() {
    let guards = std::array::from_fn::<_, 5, _>(|_| ExprId::fresh().unwrap());
    let (a, b) = byte_pair();
    let (sa, sb) = slot_pair();
    let tight = StateLimits {
        max_nodes: 28,
        max_depth: 5,
    };
    let (chain, diamond) = repeated_roots!(&a, &b, &guards, tight);
    let (slot_chain, slot_diamond) = repeated_roots!(&sa, &sb, &guards, tight);
    // Original expanded-tree accounting, unchanged: raw first=22, middle=34,
    // outer=46 and diamond=46. Only representation shrinks; no metric discount.
    let raw_first = 2 + a.nodes() + b.nodes();
    let raw_middle = 2 + a.nodes() + raw_first;
    let raw_outer = 2 + a.nodes() + raw_middle;
    let raw_diamond = 2 + 2 * raw_first;
    assert_eq!(
        (raw_first, raw_middle, raw_outer, raw_diamond),
        (22, 34, 46, 46)
    );
    assert!(raw_outer > tight.max_nodes && raw_diamond > tight.max_nodes);
    for (nodes, depth) in [
        (chain.nodes(), chain.depth()),
        (slot_chain.nodes(), slot_chain.depth()),
    ] {
        assert_eq!((nodes, depth), (28, 5));
    }
    assert_eq!((diamond.nodes(), diamond.depth()), (25, 5));
    assert_eq!((slot_diamond.nodes(), slot_diamond.depth()), (25, 5));
    let byte_graph = SymbolicGraph::bytes(&chain, GraphLimits::default()).unwrap();
    let slot_graph = SymbolicGraph::slots(&slot_chain, GraphLimits::default()).unwrap();
    assert_eq!(
        byte_graph
            .nodes
            .iter()
            .filter(|n| matches!(n, SymbolicGraphNode::BytesIte { .. }))
            .count(),
        1
    );
    assert_eq!(
        slot_graph
            .nodes
            .iter()
            .filter(|n| matches!(n, SymbolicGraphNode::SlotsIte { .. }))
            .count(),
        1
    );
    assert_eq!(chain.fresh_leaves(), guards[..3].iter().cloned().collect());
    assert_eq!(
        diamond.fresh_leaves(),
        guards[2..].iter().cloned().collect()
    );
    let mut oracle = Valuation::default();
    let expected_a = oracle.byte_array(&a);
    let expected_b = oracle.byte_array(&b);
    let slot_a = oracle.slot_array(&sa);
    let slot_b = oracle.slot_array(&sb);
    for assignment in 0..4usize.pow(5) {
        oracle.assign(&guards, assignment);
        let [g, h, k, n, m] = guards.each_ref().map(|v| oracle.word(v) != U256::ZERO);
        for (bytes, slots, take_a) in [
            (&chain, &slot_chain, g || h || k),
            (&diamond, &slot_diamond, if n { m } else { k }),
        ] {
            assert_eq!(
                oracle.byte_array(bytes),
                *if take_a { &expected_a } else { &expected_b }
            );
            assert_eq!(
                oracle.slot_array(slots),
                *if take_a { &slot_a } else { &slot_b }
            );
        }
    }
}

#[test]
fn shared_branch_scalar_and_state_limits_and_cancellation_leave_inputs_unchanged() {
    let a = ByteStateId::exact(Arc::from([7, 11]));
    let b = ByteStateId::exact(Arc::from([7, 12]));
    let sa = SlotStateId::zero().store(c(1), c(7), limits()).unwrap();
    let sb = SlotStateId::zero().store(c(1), c(8), limits()).unwrap();
    let x = ExprId::fresh().unwrap();
    let y = ExprId::fresh().unwrap();
    let t = ExprId::ite(x, c(2), c(0), limits().into()).unwrap();
    let s = ExprId::ite(y, c(1), c(0), limits().into()).unwrap();
    let deep_s = ExprId::ite(ExprId::fresh().unwrap(), s.clone(), c(0), limits().into()).unwrap();
    let inner = a.join_guarded(&b, t.clone(), limits()).unwrap();
    let slot_inner = sa.join_guarded(&sb, t.clone(), limits()).unwrap();
    let scalar_slots = SlotStateId::zero()
        .join_guarded(&SlotStateId::input(SourceId::fresh().unwrap()), t, limits())
        .unwrap();
    let before = (
        a.kind().clone(),
        b.kind().clone(),
        inner.kind().clone(),
        sa.kind().clone(),
        sb.kind().clone(),
        slot_inner.kind().clone(),
        scalar_slots.kind().clone(),
    );
    // Byte children fit at 7/3; the derived guard fits at 10/3, but the
    // resulting byte root needs 13/4. Slot roots need 19/4 and fit at 13/3.
    assert_eq!((inner.nodes(), inner.depth()), (7, 3));
    assert_eq!((slot_inner.nodes(), slot_inner.depth()), (13, 3));
    for (root, other) in [(&inner, &a), (&inner, &b)] {
        let denied = StateLimits {
            max_nodes: root.nodes() - 1,
            ..limits()
        };
        assert_eq!(
            root.join_guarded(other, s.clone(), denied),
            Err(StateError::Limit {
                nodes: root.nodes(),
                depth: root.depth()
            })
        );
    }
    assert_eq!(
        slot_inner.join_guarded(
            &sa,
            s.clone(),
            StateLimits {
                max_nodes: 12,
                ..limits()
            }
        ),
        Err(StateError::Limit {
            nodes: 13,
            depth: 3
        })
    );
    for denied in [
        StateLimits {
            max_nodes: 12,
            ..limits()
        },
        StateLimits {
            max_depth: 3,
            ..limits()
        },
    ] {
        assert!(matches!(
            inner.join_guarded(&a, s.clone(), denied),
            Err(StateError::Limit { .. })
        ));
    }
    for denied in [
        StateLimits {
            max_nodes: 18,
            ..limits()
        },
        StateLimits {
            max_depth: 3,
            ..limits()
        },
    ] {
        assert!(matches!(
            slot_inner.join_guarded(&sa, s.clone(), denied),
            Err(StateError::Limit { .. })
        ));
    }
    // A rejected optional factor falls back to the original complete shape.
    // Its actual raw state bound, not the skipped guard's bound, is reported.
    for denied in [
        StateLimits {
            max_nodes: 12,
            ..limits()
        },
        StateLimits {
            max_depth: 3,
            ..limits()
        },
    ] {
        assert_eq!(
            inner.join_guarded(&a, deep_s.clone(), denied),
            Err(StateError::Limit {
                nodes: 16,
                depth: 4
            })
        );
        assert_eq!(
            scalar_slots.join_guarded(&SlotStateId::zero(), deep_s.clone(), denied),
            Err(StateError::Limit {
                nodes: 17,
                depth: 4
            })
        );
    }
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            inner.join_guarded(&a, s.clone(), limits()),
            Err(StateError::Cancelled)
        );
        assert_eq!(
            slot_inner.join_guarded(&sa, s.clone(), limits()),
            Err(StateError::Cancelled)
        );
    });
    assert_eq!(
        (
            a.kind().clone(),
            b.kind().clone(),
            inner.kind().clone(),
            sa.kind().clone(),
            sb.kind().clone(),
            slot_inner.kind().clone(),
            scalar_slots.kind().clone(),
        ),
        before
    );
    let normalized = inner.join_guarded(&a, s.clone(), limits()).unwrap();
    let normalized_slots = slot_inner.join_guarded(&sa, s, limits()).unwrap();
    assert_eq!((normalized.nodes(), normalized.depth()), (13, 4));
    assert_eq!(
        (normalized_slots.nodes(), normalized_slots.depth()),
        (19, 4)
    );
}

#[test]
fn optional_factoring_preserves_valid_original_depth_three_and_four_shapes() {
    let a = ByteStateId::exact(Arc::from([7, 11]));
    let b = ByteStateId::exact(Arc::from([7, 12]));
    let outer = ExprId::ite(ExprId::fresh().unwrap(), c(1), c(2), limits().into()).unwrap();
    assert_eq!(outer.depth(), 2);
    let t = ExprId::fresh().unwrap();
    let h = ExprId::fresh().unwrap();
    let left = a.join_guarded(&b, t.clone(), limits()).unwrap();
    let right = a.join_guarded(&b, h.clone(), limits()).unwrap();
    let original = (
        a.kind().clone(),
        b.kind().clone(),
        left.kind().clone(),
        right.kind().clone(),
    );
    // Every raw shape has depth 3; its factored scalar guard would have depth
    // 3, making the replacement state depth 4. Decline only the optimization.
    for (yes, no) in [
        (&left, &a),
        (&left, &b),
        (&a, &left),
        (&b, &left),
        (&left, &right),
    ] {
        let result = yes
            .join_guarded(
                no,
                outer.clone(),
                StateLimits {
                    max_depth: 3,
                    ..limits()
                },
            )
            .unwrap();
        assert_eq!(result.depth(), 3);
        assert!(
            matches!(result.kind(), ByteStateKind::Ite { condition, then_state, else_state }
            if condition == &outer && then_state == yes && else_state == no)
        );
    }
    assert_eq!(
        (
            a.kind().clone(),
            b.kind().clone(),
            left.kind().clone(),
            right.kind().clone()
        ),
        original
    );

    let sa = SlotStateId::zero().store(c(1), c(7), limits()).unwrap();
    let sb = SlotStateId::zero().store(c(1), c(8), limits()).unwrap();
    let slot_outer = ExprId::ite(ExprId::fresh().unwrap(), outer, c(0), limits().into()).unwrap();
    assert_eq!(slot_outer.depth(), 3);
    let left = sa.join_guarded(&sb, t, limits()).unwrap();
    let right = sa.join_guarded(&sb, h, limits()).unwrap();
    let original = (
        sa.kind().clone(),
        sb.kind().clone(),
        left.kind().clone(),
        right.kind().clone(),
    );
    for (yes, no) in [
        (&left, &sa),
        (&left, &sb),
        (&sa, &left),
        (&sb, &left),
        (&left, &right),
    ] {
        let result = yes
            .join_guarded(
                no,
                slot_outer.clone(),
                StateLimits {
                    max_depth: 4,
                    ..limits()
                },
            )
            .unwrap();
        assert_eq!(result.depth(), 4);
        assert!(
            matches!(result.kind(), SlotStateKind::Ite { condition, then_state, else_state }
            if condition == &slot_outer && then_state == yes && else_state == no)
        );
    }
    assert_eq!(
        (
            sa.kind().clone(),
            sb.kind().clone(),
            left.kind().clone(),
            right.kind().clone()
        ),
        original
    );
}
