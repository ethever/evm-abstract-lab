//! Borrow already public canonical bounds; never reconstruct executable roots.
use super::{Pools, api};
use evm_abstract::{
    domain::WordElement,
    world::{
        Code, EvmByteArraySummary, EvmByteContentView, EvmPlaneSummaryView, EvmStoreSummary,
        Existence, OrderedMap,
    },
};

pub(super) fn bytes<N: WordElement>(
    source: &EvmByteArraySummary<N>,
    pools: &mut Pools<'_, N>,
) -> Result<api::MinimumByteArray, api::ApiError> {
    pools.symbolic.charge(4)?;
    let source = source.view();
    let content = match source.content {
        EvmByteContentView::Zero => api::MinimumByteContent::Zero,
        EvmByteContentView::Exact(bytes) => {
            pools.symbolic.payload(bytes.len())?;
            pools.symbolic.charge(bytes.len())?;
            api::MinimumByteContent::Exact(alloy_primitives::hex::encode_prefixed(bytes))
        }
        EvmByteContentView::Cells(cells) => {
            pools.symbolic.charge(cells.len())?;
            let mut out = Vec::with_capacity(cells.len());
            for (offset, value) in cells {
                out.push(api::MinimumByteCell {
                    offset: *offset,
                    value: pools.symbolic.value(value)?,
                });
            }
            api::MinimumByteContent::Cells(out)
        }
    };
    pools.symbolic.charge(source.words.len())?;
    let mut words = Vec::with_capacity(source.words.len());
    for (offset, value) in source.words {
        words.push(api::MinimumByteCell {
            offset: *offset,
            value: pools.symbolic.value(value)?,
        });
    }
    Ok(api::MinimumByteArray {
        length: pools.symbolic.value(source.length)?,
        memory: source.memory,
        content,
        words,
    })
}

fn owners<N: WordElement>(
    source: &OrderedMap<evm_abstract::Address, evm_abstract::domain::EvmValue<N>>,
    pools: &mut Pools<'_, N>,
) -> Result<Vec<api::MinimumOwnerValue>, api::ApiError> {
    pools.symbolic.charge(source.len().saturating_mul(48))?;
    source
        .iter()
        .map(|(owner, value)| {
            Ok(api::MinimumOwnerValue {
                owner: owner.to_string(),
                value: pools.symbolic.value(value)?,
            })
        })
        .collect()
}

fn flags<N: WordElement>(
    source: &OrderedMap<evm_abstract::Address, Option<bool>>,
    pools: &mut Pools<'_, N>,
) -> Result<Vec<api::MinimumOwnerFlag>, api::ApiError> {
    pools.symbolic.charge(source.len().saturating_mul(48))?;
    source
        .iter()
        .map(|(owner, value)| {
            pools.symbolic.charge(0)?;
            Ok(api::MinimumOwnerFlag {
                owner: owner.to_string(),
                value: *value,
            })
        })
        .collect()
}

fn plane<N: WordElement>(
    source: EvmPlaneSummaryView<'_, N>,
    pools: &mut Pools<'_, N>,
) -> Result<api::MinimumPlane, api::ApiError> {
    pools.symbolic.charge(
        source
            .slots
            .len()
            .saturating_mul(112)
            .saturating_add(source.independent_slots.len().saturating_mul(112))
            .saturating_add(source.independent_owners.len().saturating_mul(48))
            .saturating_add(8),
    )?;
    let mut slots = Vec::with_capacity(source.slots.len());
    for ((owner, slot), value) in source.slots {
        slots.push(api::MinimumSlot {
            owner: owner.to_string(),
            slot: super::value::word(*slot),
            value: pools.symbolic.value(value)?,
        });
    }
    let mut independent_slots = Vec::with_capacity(source.independent_slots.len());
    for ((owner, slot), value) in source.independent_slots {
        pools.symbolic.charge(0)?;
        independent_slots.push(api::MinimumSlotIndependence {
            owner: owner.to_string(),
            slot: super::value::word(*slot),
            independent: *value,
        });
    }
    let mut independent_owners = Vec::with_capacity(source.independent_owners.len());
    for (owner, value) in source.independent_owners {
        pools.symbolic.charge(0)?;
        independent_owners.push(api::MinimumOwnerFlag {
            owner: owner.to_string(),
            value: Some(*value),
        });
    }
    Ok(api::MinimumPlane {
        default: pools.symbolic.value(source.global_default)?,
        owners: owners(source.defaults, pools)?,
        slots,
        independent_default: source.independent_default,
        independent_owners,
        independent_slots,
    })
}

pub(super) fn store<N: WordElement>(
    source: &EvmStoreSummary<N>,
    pools: &mut Pools<'_, N>,
) -> Result<api::MinimumStore, api::ApiError> {
    pools.symbolic.charge(16)?;
    let view = source.view();
    pools.symbolic.charge(view.codes.len())?;
    let mut codes = Vec::with_capacity(view.codes.len());
    for (owner, source) in view.codes {
        codes.push(code(*owner, Some(*owner), source, pools)?);
    }
    pools
        .symbolic
        .charge(view.existence.len().saturating_mul(48))?;
    let mut existence = Vec::with_capacity(view.existence.len());
    for (owner, value) in view.existence {
        pools.symbolic.charge(0)?;
        existence.push(api::MinimumExistence {
            owner: owner.to_string(),
            value: match value {
                Existence::Unknown => api::AccountExistence::Unknown,
                Existence::Present => api::AccountExistence::Present,
                Existence::Absent => api::AccountExistence::Absent,
            },
        });
    }
    pools
        .symbolic
        .charge(source.logs_len().saturating_mul(192))?;
    let mut logs = Vec::with_capacity(source.logs_len());
    for (key, topics, data) in source.logs() {
        pools.symbolic.charge(topics.len())?;
        logs.push(api::MinimumLog {
            address: key.address.to_string(),
            code_address: key.code_address.to_string(),
            code_hash: key.code_hash.to_string(),
            source_kind: match key.source_kind {
                evm_abstract::world::LogSourceKind::Init => api::LogSourceKind::Init,
                evm_abstract::world::LogSourceKind::Runtime => api::LogSourceKind::Runtime,
            },
            pc: key.pc,
            topics: topics
                .iter()
                .map(|topic| pools.symbolic.value(topic))
                .collect::<Result<_, _>>()?,
            data: bytes(data, pools)?,
        });
    }
    Ok(api::MinimumStore {
        persistent: plane(view.persistent, pools)?,
        transient: plane(view.transient, pools)?,
        balance_default: pools.symbolic.value(view.balance_default)?,
        balances: owners(view.balances, pools)?,
        nonces: owners(view.nonces, pools)?,
        codes,
        existence,
        created_default: view.created_default,
        created: flags(view.created, pools)?,
        pending_destruction_default: view.pending_destruction_default,
        pending_destruction: flags(view.pending_destruction, pools)?,
        touched_default: view.touched_default,
        touched: flags(view.touched, pools)?,
        logs,
        logs_unknown: view.logs_unknown,
    })
}

pub(super) fn code<N: WordElement>(
    owner: evm_abstract::Address,
    source_address: Option<evm_abstract::Address>,
    source: &Code,
    pools: &mut Pools<'_, N>,
) -> Result<api::MinimumCode, api::ApiError> {
    pools.symbolic.charge(96)?;
    let (kind, program, delegation_target) = match source {
        Code::Runtime(program) => {
            pools.symbolic.charge(program.byte_len())?;
            (
                api::CodeKind::Runtime,
                Some(pools.program_identity(
                    source_address,
                    alloy_primitives::keccak256(program.bytes()),
                    api::CodeKind::Runtime,
                    program,
                )?),
                None,
            )
        }
        Code::Delegation(target) => (api::CodeKind::Delegation, None, Some(target.to_string())),
        Code::Empty => (api::CodeKind::Empty, None, None),
        Code::Unknown => (api::CodeKind::Unknown, None, None),
    };

    Ok(api::MinimumCode {
        owner: owner.to_string(),
        kind,
        program,
        delegation_target,
    })
}
