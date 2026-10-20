//! Fixed complete-word coordinates retain predicates absent from byte exports.

use super::{COPY_FACT_LIMIT, EvmByteArray, EvmValue, RangeError, join_scalar, scalar};
use crate::domain::{
    AlgebraRef, EvmNumericAlgebra, WordElement,
    symbolic::{ByteStateId, ByteStateKind, ExprError, ExprId, ExprKind},
};
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

// Use the existing retained-byte envelope, with 32 bytes per complete demand.
const WORDS: usize = COPY_FACT_LIMIT / 32;

pub(super) fn needed<N: WordElement>(array: &EvmByteArray<N>) -> bool {
    array
        .operands
        .values()
        .chain([&array.length, &array.default])
        .any(|value| value.numeric().requires_retained_binding())
}

fn insert(out: &mut BTreeSet<usize>, index: usize) {
    if index.checked_add(31).is_some() {
        out.insert(index);
        if out.len() > WORDS {
            out.pop_last();
        }
    }
}

fn translated(out: &mut BTreeSet<usize>, source: &BTreeSet<usize>, from: &ExprId, to: U256) {
    let Some(from) = from.as_constant() else {
        return;
    };
    for index in source {
        if let Some(index) = U256::from(*index)
            .checked_sub(from)
            .and_then(|relative| to.checked_add(relative))
            .and_then(|index| usize::try_from(index).ok())
        {
            insert(out, index);
        }
    }
}

/// Iterative history walk, bounded by retained graph shape and fixed windows.
/// Candidate coordinates alone carry no guarantees: each is evaluated against
/// the CURRENT root, so overwritten, padded and possibly aliased words decline.
fn coordinates(root: &ByteStateId) -> BTreeSet<usize> {
    let mut pending = vec![(root, false)];
    let mut done = BTreeMap::<u64, BTreeSet<usize>>::new();
    while let Some((state, visited)) = pending.pop() {
        if done.contains_key(&state.id()) {
            continue;
        }
        if !visited {
            pending.push((state, true));
            match state.kind() {
                ByteStateKind::WriteByte { previous, .. }
                | ByteStateKind::WriteWord { previous, .. } => pending.push((previous, false)),
                ByteStateKind::Copy {
                    previous, source, ..
                } => {
                    pending.extend([(previous, false), (&source.state, false)]);
                }
                ByteStateKind::Slice { source } => pending.push((&source.state, false)),
                ByteStateKind::Ite {
                    then_state,
                    else_state,
                    ..
                } => {
                    pending.extend([(then_state, false), (else_state, false)]);
                }
                _ => {}
            }
            continue;
        }
        let mut out = BTreeSet::new();
        let mut previous = |state: &ByteStateId| {
            for index in &done[&state.id()] {
                insert(&mut out, *index);
            }
        };
        match state.kind() {
            ByteStateKind::WriteByte {
                previous: state, ..
            } => previous(state),
            ByteStateKind::WriteWord {
                previous: state,
                offset,
                ..
            } => {
                previous(state);
                if let Some(index) = offset
                    .as_constant()
                    .and_then(|word| usize::try_from(word).ok())
                {
                    insert(&mut out, index);
                }
            }
            ByteStateKind::Copy {
                previous: state,
                destination,
                source,
                ..
            } => {
                previous(state);
                if let Some(destination) = destination.as_constant() {
                    translated(
                        &mut out,
                        &done[&source.state.id()],
                        &source.offset,
                        destination,
                    );
                }
            }
            ByteStateKind::Slice { source } => {
                translated(
                    &mut out,
                    &done[&source.state.id()],
                    &source.offset,
                    U256::ZERO,
                );
            }
            ByteStateKind::Ite {
                then_state,
                else_state,
                ..
            } => {
                previous(then_state);
                previous(else_state);
            }
            _ => {}
        }
        done.insert(state.id(), out);
    }
    done.remove(&root.id()).expect("root visited")
}

pub(super) fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
) -> Result<BTreeMap<usize, EvmValue<N>>, RangeError> {
    if !needed(array) {
        return Ok(BTreeMap::new());
    }
    let mut coordinates = coordinates(&array.root);
    // An instantiated summary has a fresh observed root rather than writes.
    // Its exact read facts provide candidate coordinates for the next summary.
    let mut candidate = |expression: &ExprId| {
        if let ExprKind::WordRead { view, offset } = expression.kind()
            && let Some(index) = view
                .offset
                .as_constant()
                .zip(offset.as_constant())
                .and_then(|(base, relative)| base.checked_add(relative))
                .and_then(|index| usize::try_from(index).ok())
        {
            insert(&mut coordinates, index);
        }
    };
    for expression in array.operands.keys() {
        candidate(expression);
    }
    array
        .conditional
        .visit(&mut |expression, _| candidate(expression));
    let mut out = BTreeMap::new();
    for index in coordinates {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        let value = array.read_word_checked_with(&EvmValue::constant(U256::from(index)), domain)?;
        if value.numeric().requires_retained_binding()
            && !value.numeric().is_top()
            && value.singleton().is_none()
        {
            out.insert(index, scalar(&value, domain));
        }
    }
    Ok(out)
}

pub(super) fn join<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &BTreeMap<usize, EvmValue<N>>,
    right: &BTreeMap<usize, EvmValue<N>>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
) -> BTreeMap<usize, EvmValue<N>> {
    left.iter()
        .filter_map(|(index, value)| {
            let other = right.get(index)?;
            let value = join_scalar(value, other, domain, widen);
            (!value.numeric().is_top()).then_some((*index, value))
        })
        .collect()
}

pub(super) fn instantiate<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    words: &BTreeMap<usize, EvmValue<N>>,
    out: &mut EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
) -> Result<(), RangeError> {
    for (index, value) in words {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        let expression = super::super::word_plan::expression(
            &out.root,
            (!out.memory).then_some(&out.length),
            &ExprId::constant(U256::from(*index)),
            &super::super::conditional::Context::new(&out.operands, Some(&out.conditional), domain),
            domain,
        )?;
        out.operands.insert(
            expression.clone(),
            value.clone().with_expression(expression),
        );
    }
    Ok(())
}

pub(super) fn work<N: WordElement>(
    words: &BTreeMap<usize, EvmValue<N>>,
    root: usize,
    length: usize,
) -> usize {
    words.values().fold(0usize, |work, value| {
        work.saturating_add(
            root.saturating_add(length)
                .saturating_add(8)
                .saturating_mul(2),
        )
        .saturating_add(value.work_size().saturating_mul(3))
        .saturating_add(value.singleton_work())
        .saturating_add(value.clone_work())
    })
}
