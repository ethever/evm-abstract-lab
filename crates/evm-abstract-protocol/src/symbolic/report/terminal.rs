//! Structural binding validation, not an EVM semantic proof checker. In
//! particular widened input DTOs never establish a broader all-input claim.
use super::Validation;
use crate::{
    EnvironmentSnapshot, Expression, OutcomeKind, ReportValidationFailure, RootTerminalCoverage,
    RootTerminalFrameCode, RootTerminalSnapshot, RootTerminalValidationFailure as Failure,
    SymbolicSort,
};

fn invalid(kind: Failure) -> ReportValidationFailure {
    ReportValidationFailure::RootTerminal(kind)
}
fn hex(text: &str, max_digits: usize) -> bool {
    text.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= max_digits
            && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
fn zero(text: &str) -> bool {
    hex(text, 64) && text[2..].bytes().all(|byte| byte == b'0')
}
impl Validation<'_> {
    pub(super) fn environment(
        &self,
        env: &EnvironmentSnapshot,
    ) -> Result<(), ReportValidationFailure> {
        self.buffer(env.calldata)?;
        for value in [
            &env.call_value,
            &env.gas_price,
            &env.timestamp,
            &env.number,
            &env.prevrandao,
            &env.gas_limit,
            &env.chain_id,
            &env.base_fee,
            &env.blob_base_fee,
            &env.blob_count,
        ] {
            self.value(value)?;
        }
        Ok(())
    }
    pub(super) fn terminal(
        &self,
        source: &RootTerminalCoverage,
    ) -> Result<(), ReportValidationFailure> {
        let input = &source.input;
        self.root(SymbolicSort::Word, input.storage_source.node)?;
        self.program(Some(input.program))?;
        self.buffer(input.calldata)?;
        self.store(input.saved_store)?;
        self.store(input.initial_store)?;
        self.value(&input.call_value)?;
        self.environment(&input.environment)?;
        if let crate::RootTerminalTrigger::MandatoryRetention(trigger) = &source.trigger {
            if !self
                .report
                .states
                .iter()
                .any(|state| state.state == trigger.state)
            {
                return Err(ReportValidationFailure::Pool(crate::PoolReferenceFailure {
                    kind: crate::PoolReferenceKind::MachineState,
                    index: trigger.state,
                    length: self.report.states.len(),
                }));
            }
            // Core captures this exact exported site before flattening. A
            // procedure site need not name an opcode in the root program;
            // public frontier categories cannot reconstruct the raw cause.
            if !self.report.frontiers.iter().any(|frontier| {
                frontier.from == Some(trigger.state) && frontier.pc == Some(trigger.pc)
            }) {
                return Err(invalid(Failure::RetentionSite));
            }
        }
        let metadata = &self.report.metadata;
        match (&input.snapshot, &metadata.snapshot) {
            (RootTerminalSnapshot::Offline(_), None) => {}
            (RootTerminalSnapshot::Chain(snapshot), Some(expected))
                if snapshot.chain_id == expected.chain_id
                    && snapshot.block_hash == expected.block_hash
                    && hex(&snapshot.chain_id, 64)
                    && hex(&snapshot.block_hash, 64)
                    && snapshot.block_hash.len() == 66 => {}
            _ => return Err(invalid(Failure::Snapshot)),
        }
        if input.fork != self.report.fork {
            return Err(invalid(Failure::Fork));
        }
        if input.environment != metadata.environment {
            return Err(invalid(Failure::Environment));
        }
        let config = &input.config;
        let limits = &metadata.limits;
        if !config.root_terminal_coverage
            || !limits.root_terminal_coverage
            || config.domain_profile != limits.domain_profile
            || config.reduction_rounds != limits.reduction_rounds
            || config.max_facts != limits.max_facts
            || config.relations.enabled != limits.relations_enabled
            || config.relations.max_constraints != limits.max_constraints
            || config.relations.max_nodes != limits.max_expression_nodes
            || config.relations.max_depth != limits.max_expression_depth
            || config.max_constants != limits.max_constants
            || config.context_depth != limits.context_depth
            || config.max_states != limits.max_states
            || config.max_transfers != limits.max_transfers
            || config.max_work != limits.max_work
            || config.max_call_depth != limits.max_call_depth
            || config.max_memory_bytes != limits.max_memory_bytes
            || config.use_summaries != limits.use_summaries
            || [
                config.max_constants,
                config.reduction_rounds,
                config.max_facts,
                config.max_states,
                config.max_transfers,
                config.max_work,
                config.max_call_depth,
                config.max_memory_bytes,
                config.relations.max_constraints,
                config.relations.max_nodes,
                config.relations.max_depth,
            ]
            .contains(&0)
        {
            return Err(invalid(Failure::Configuration));
        }
        let domain = &input.domain_spec;
        if domain.schema_version == 0
            || domain.word_bits != 256
            || domain.cost_version == 0
            || domain.provenance_policy.is_empty()
            || domain.profile != config.domain_profile
            || domain.capacity != config.max_constants
            || domain.reduction_rounds != config.reduction_rounds
            || domain.fact_limit != config.max_facts
            || domain.relations != config.relations
        {
            return Err(invalid(Failure::DomainSpec));
        }
        let frame = &input.frame;
        // Pool membership was checked above. A captured executable is never
        // recovered from a possibly havoced current account's code field.
        let mut programs = self
            .report
            .programs
            .iter()
            .filter(|program| program.id == input.program);
        let program = programs.next().ok_or_else(|| invalid(Failure::Frame))?;
        if programs.next().is_some() {
            return Err(invalid(Failure::Frame));
        }
        if input.entry_address != metadata.entry_address
            || frame.address != input.entry_address
            || frame.address_value != input.environment.to
            || frame.caller != input.environment.caller
            || frame.is_static != input.environment.is_static
            || program.code_address != frame.code_address
            || program.code_hash != frame.code_hash
            || frame.mode != RootTerminalFrameCode::Runtime
            || program.kind != crate::CodeKind::Runtime
            || metadata.root_program != Some(input.program)
            || program.blocks.is_empty()
            || frame.basic_block_index != 0
            || frame.stack_height != 0
            || !frame.jump_history.is_empty()
        {
            return Err(invalid(Failure::Frame));
        }
        if let Some(identity) = &input.call_value.identity
            && Some(identity.scope) != input.input_scope
        {
            return Err(invalid(Failure::InputIdentity));
        }
        if let Some(expression) = input.call_value.expression
            && let Expression::Input(identity) = &self.graph.nodes[expression.node]
            && Some(identity.scope) != input.input_scope
        {
            return Err(invalid(Failure::InputIdentity));
        }
        for (outcome, expected) in source.outcomes.iter().zip([
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ]) {
            if outcome.kind != expected {
                return Err(invalid(Failure::OutcomeOrder));
            }
            self.buffer(outcome.data)?;
            self.store(outcome.store)?;
        }
        let failure = &self.report.byte_arrays[source.outcomes[2].data];
        // Sparse cells outside the length are irrelevant; arbitrary Return and
        // Revert views need not be enumerated or exact-cell arrays.
        if !zero(&failure.length.unsigned.lower) || !zero(&failure.length.unsigned.upper) {
            return Err(invalid(Failure::FailureData));
        }
        Ok(())
    }
}
