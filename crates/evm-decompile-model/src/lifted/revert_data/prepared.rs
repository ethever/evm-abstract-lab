//! Local prepared-consumer shape; byte equality and epoch replay remain core
//! obligations. Prefix COPY is never owned or moved by this source interval.
use crate::{
    Block, Effects, EntityId, LiftedByteSegment, LiftedByteSource, LiftedRevertData, Operation,
};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::lifted) fn dependencies(
    block: &Block,
    data: &LiftedRevertData,
) -> BTreeSet<EntityId> {
    let definitions: BTreeMap<_, _> = block
        .operations
        .iter()
        .flat_map(|o| o.outputs.iter().map(move |v| (*v, o)))
        .collect();
    let mut values = vec![data.offset, data.size];
    let mut seen = BTreeSet::new();
    let mut needed = BTreeSet::new();
    while let Some(value) = values.pop() {
        if !seen.insert(value) {
            continue;
        }
        if let Some(op) = definitions.get(&value) {
            needed.insert(op.id);
            values.extend(&op.inputs);
        }
    }
    needed
}
pub(in crate::lifted) fn operation(
    op: &Operation,
    data: &LiftedRevertData,
    needed: &BTreeSet<EntityId>,
) -> bool {
    if op.id == data.operation {
        return op.opcode == 0xfd && crate::lifted::payload::capsule_operation(op);
    }
    if op.opcode == 0x3d {
        return needed.contains(&op.id)
            && op.inputs.is_empty()
            && op.outputs.len() == 1
            && op.memory_input.is_none()
            && op.memory_output.is_none()
            && op.effect_input.is_some()
            && op.effect_output.is_some()
            && op.effects
                == Effects {
                    return_data_read: true,
                    ..Effects::default()
                };
    }
    op.effects.is_pure()
        && crate::lifted::payload::capsule_operation(op)
        && [
            op.memory_input,
            op.memory_output,
            op.effect_input,
            op.effect_output,
        ]
        .iter()
        .all(Option::is_none)
}
pub(in crate::lifted) fn data_shape(data: &LiftedRevertData) -> bool {
    matches!(
        data.content.segments(),
        [LiftedByteSegment::Slice {
            source: LiftedByteSource::ReturnData(_),
            ..
        }]
    ) && data.writes.len() == 1
}
