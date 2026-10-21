//! Project only the engine's immutable record. Flattened frontiers never
//! authorize construction of a certificate in this adapter.
use super::{Pools, api, environment, value};
use evm_abstract::{
    Fork,
    analysis::{self, FrameCode},
    domain::{WordElement, relational::RelationLimits},
    world::SnapshotIdentity,
};

fn relations(source: RelationLimits) -> api::RootTerminalRelationLimits {
    api::RootTerminalRelationLimits {
        enabled: source.enabled,
        max_constraints: source.max_constraints as u64,
        max_nodes: source.max_nodes as u64,
        max_depth: source.max_depth as u64,
    }
}
pub(super) fn project<N: WordElement>(
    source: &analysis::EvmRootTerminalCoverage<N>,
    numeric_model: api::NumericModel,
    pools: &mut Pools<'_, N>,
) -> Result<api::RootTerminalCoverage, api::ApiError> {
    pools.symbolic.charge(1)?;
    let input = source.input();
    let snapshot = match input.snapshot().identity() {
        SnapshotIdentity::Offline { label } => {
            pools.symbolic.payload(label.len())?;
            let namespace = input
                .snapshot()
                .offline_namespace()
                .ok_or_else(|| api::ApiError {
                    code: api::ApiErrorCode::Internal,
                    message: "native offline snapshot has no namespace".into(),
                    details: api::ErrorDetails::Report(api::ReportValidationFailure::RootTerminal(
                        api::RootTerminalValidationFailure::Snapshot,
                    )),
                })?;
            api::RootTerminalSnapshot::Offline(api::RootTerminalOfflineSnapshot {
                label: label.clone(),
                namespace: api::DecimalU64(namespace),
            })
        }
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => api::RootTerminalSnapshot::Chain(api::RootTerminalChainSnapshot {
            chain_id: value::word(*chain_id),
            block_hash: block_hash.to_string(),
        }),
    };
    let config = input.config();
    let domain = input.domain_spec();
    let frame = input.frame();
    pools.symbolic.charge(frame.jump_history.len())?;
    pools.symbolic.payload(domain.provenance_policy().len())?;
    let program = pools.program(
        frame.code_address,
        frame.code_hash,
        super::super::pools::mode(frame.mode),
        input.program(),
    )?;
    let input = api::RootTerminalInputBinding {
        snapshot,
        revision: api::DecimalU64(input.revision().value()),
        storage_source: pools.symbolic.expression(input.storage_source())?,
        input_scope: input.input_scope().map(api::DecimalU64),
        fork: match input.fork() {
            Fork::Cancun => api::Fork::Cancun,
            Fork::Prague => api::Fork::Prague,
            Fork::Osaka => api::Fork::Osaka,
        },
        entry_address: input.entry().address.to_string(),
        environment: environment::project(&input.entry().environment, pools)?,
        config: api::RootTerminalConfig {
            context_depth: config.analysis.context_depth as u64,
            max_states: config.analysis.max_states as u64,
            max_transfers: config.analysis.max_transfers as u64,
            max_work: config.max_work as u64,
            max_call_depth: config.max_call_depth as u64,
            max_memory_bytes: config.max_memory_bytes as u64,
            use_summaries: config.use_summaries,
            root_terminal_coverage: config.root_terminal_coverage,
        },
        numeric_model,
        domain_spec: api::RootTerminalDomainSpec {
            schema_version: domain.schema_version(),
            word_bits: domain.word_bits(),
            capacity: domain.capacity() as u64,
            reduction_rounds: domain.reduction_rounds() as u64,
            fact_limit: domain.fact_limit() as u64,
            cost_version: domain.cost_version(),
            widening_after_updates: domain.widening_after_updates() as u64,
            provenance_policy: domain.provenance_policy().into(),
            relations: relations(domain.relations()),
        },
        frame: api::RootTerminalFrame {
            code_address: frame.code_address.to_string(),
            code_hash: frame.code_hash.to_string(),
            mode: match frame.mode {
                FrameCode::Runtime => api::RootTerminalFrameCode::Runtime,
                FrameCode::InitCode => api::RootTerminalFrameCode::InitCode,
                FrameCode::Precompile(address) => {
                    api::RootTerminalFrameCode::Precompile(address.to_string())
                }
                FrameCode::Empty => api::RootTerminalFrameCode::Empty,
                FrameCode::InvalidDelegation => api::RootTerminalFrameCode::InvalidDelegation,
                FrameCode::Unknown => api::RootTerminalFrameCode::Unknown,
            },
            address: frame.address.to_string(),
            address_value: value::address(frame.address_value),
            caller: value::address(frame.caller),
            is_static: frame.is_static,
            basic_block_index: frame.basic_block_index,
            stack_height: frame.stack_height,
            jump_history: frame.jump_history.clone(),
        },
        program,
        calldata: pools.buffer(input.calldata())?,
        call_value: pools.symbolic.value(input.call_value())?,
        saved_store: pools.store(input.saved_store().state())?,
        initial_store: pools.store(input.initial_store())?,
    };
    let outcome = |source: &analysis::EvmRootTerminalOutcome<N>, pools: &mut Pools<'_, N>| {
        Ok::<_, api::ApiError>(api::RootTerminalOutcome {
            kind: match source.kind() {
                analysis::OutcomeKind::Return => api::OutcomeKind::Return,
                analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
                analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
            },
            data: pools.buffer(source.data())?,
            store: pools.store(source.store())?,
        })
    };
    let outcomes = source.outcomes();
    Ok(api::RootTerminalCoverage {
        scope: match source.scope() {
            analysis::RootTerminalScope::AllInputStates => api::RootTerminalScope::AllInputStates,
        },
        input,
        outcomes: [
            outcome(&outcomes[0], pools)?,
            outcome(&outcomes[1], pools)?,
            outcome(&outcomes[2], pools)?,
        ],
        trigger: match source.trigger() {
            analysis::RootTerminalTrigger::Work => api::RootTerminalTrigger::Work,
            analysis::RootTerminalTrigger::SummaryWork => api::RootTerminalTrigger::SummaryWork,
            analysis::RootTerminalTrigger::States => api::RootTerminalTrigger::States,
            analysis::RootTerminalTrigger::Transfers => api::RootTerminalTrigger::Transfers,
            analysis::RootTerminalTrigger::MandatoryRetention { state, pc, kind } => {
                api::RootTerminalTrigger::MandatoryRetention(api::RootTerminalRetentionTrigger {
                    state: *state,
                    pc: *pc,
                    kind: match kind {
                        analysis::RootTerminalRetention::Expression => {
                            api::RootTerminalRetention::Expression
                        }
                        analysis::RootTerminalRetention::State => api::RootTerminalRetention::State,
                    },
                })
            }
        },
    })
}

#[cfg(test)]
mod tests;
