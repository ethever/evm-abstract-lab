//! Every graph and pool reference is checked before a consumer follows it.
mod hybrid;
mod process;
mod regional;
mod source;
mod terminal;
use crate::{
    AccountState, AnalysisGraph, AnalysisReport, MachineSnapshot, PoolReferenceFailure,
    PoolReferenceKind, ReportValidationFailure, SymbolicGraph, SymbolicReference, SymbolicSort,
    ValueInfo,
};

struct Validation<'a> {
    report: &'a AnalysisGraph,
    programs: &'a [crate::ProgramInfo],
    graph: &'a SymbolicGraph,
}
impl Validation<'_> {
    fn root(&self, sort: SymbolicSort, node: usize) -> Result<(), ReportValidationFailure> {
        self.graph
            .reference(None, SymbolicReference { sort, node })
            .map_err(ReportValidationFailure::Symbolic)
    }
    fn value(&self, value: &ValueInfo) -> Result<(), ReportValidationFailure> {
        if let Some(expr) = value.expression {
            self.root(SymbolicSort::Word, expr.node)?;
            if let (Some(identity), crate::Expression::Input(expression)) =
                (&value.identity, &self.graph.nodes[expr.node])
                && identity != expression
            {
                return Err(ReportValidationFailure::RootTerminal(
                    crate::RootTerminalValidationFailure::InputIdentity,
                ));
            }
        }
        Ok(())
    }
    fn pool(
        &self,
        kind: PoolReferenceKind,
        index: usize,
        length: usize,
    ) -> Result<(), ReportValidationFailure> {
        if index >= length {
            return Err(ReportValidationFailure::Pool(PoolReferenceFailure {
                kind,
                index,
                length,
            }));
        }
        Ok(())
    }
    fn buffer(&self, id: usize) -> Result<(), ReportValidationFailure> {
        self.pool(
            PoolReferenceKind::ByteArray,
            id,
            self.report.byte_arrays.len(),
        )
    }
    fn store(&self, id: usize) -> Result<(), ReportValidationFailure> {
        self.pool(PoolReferenceKind::Store, id, self.report.stores.len())
    }
    fn program(&self, id: Option<usize>) -> Result<(), ReportValidationFailure> {
        if let Some(id) = id {
            // ProgramInfo.id is a report-local identity, not necessarily its
            // position; consumers preserve sparse native presentation IDs.
            let program = self.programs.iter().find(|program| program.id == id);
            if program.is_none() {
                return Err(ReportValidationFailure::Pool(PoolReferenceFailure {
                    kind: PoolReferenceKind::Program,
                    index: id,
                    length: self.programs.len(),
                }));
            }
            if program.is_some_and(|program| program.fork != self.report.fork) {
                return Err(ReportValidationFailure::Source(
                    crate::SourceValidationKind::Fork,
                ));
            }
        }
        Ok(())
    }
    fn pending(&self, id: usize) -> Result<(), ReportValidationFailure> {
        self.pool(
            PoolReferenceKind::PendingMachine,
            id,
            self.report.pending_machines.len(),
        )
    }
    fn account(&self, account: &AccountState) -> Result<(), ReportValidationFailure> {
        self.program(account.program)?;
        self.root(SymbolicSort::Slots, account.storage_root.node)?;
        self.root(SymbolicSort::Slots, account.transient_root.node)?;
        for value in [
            &account.balance,
            &account.nonce,
            &account.storage_default,
            &account.transient_default,
        ] {
            self.value(value)?;
        }
        for slot in account.storage.iter().chain(&account.transient) {
            self.value(&slot.value)?;
        }
        Ok(())
    }
    fn machine(&self, machine: &MachineSnapshot) -> Result<(), ReportValidationFailure> {
        self.store(machine.store)?;
        self.root(SymbolicSort::Path, machine.path.node)?;
        for frame in &machine.frames {
            self.process_frame(frame)?;
            self.program(frame.program)?;
            self.store(frame.rollback_store)?;
            for id in [frame.memory, frame.calldata, frame.returndata]
                .into_iter()
                .flatten()
            {
                self.buffer(id)?;
            }
            self.value(&frame.call_value)?;
            for value in [&frame.address_operand, &frame.caller_operand]
                .into_iter()
                .flatten()
            {
                self.value(value)?;
            }
            for value in &frame.stack {
                self.value(value)?;
            }
            if let Some(next) = &frame.continuation {
                self.value(&next.output_offset)?;
                self.value(&next.output_size)?;
            }
        }
        Ok(())
    }
    fn all(&self) -> Result<(), ReportValidationFailure> {
        self.graph
            .validate()
            .map_err(ReportValidationFailure::Symbolic)?;
        self.program(self.report.metadata.root_program)?;
        self.environment(&self.report.metadata.environment)?;
        if let Some(coverage) = &self.report.root_terminal_coverage {
            self.terminal(coverage)?;
        }
        for node in &self.graph.nodes {
            let bindings = match node {
                crate::Expression::BytesInput(input) => &input.bindings,
                crate::Expression::SlotsInput(input) => &input.bindings,
                _ => continue,
            };
            for binding in bindings {
                self.value(&binding.numeric)?;
            }
        }
        for bytes in &self.report.byte_arrays {
            for fact in &bytes.symbolic_facts {
                self.root(SymbolicSort::Word, fact.expression.node)?;
                self.value(&fact.value)?;
            }
            self.value(&bytes.length)?;
            self.value(&bytes.default)?;
            self.root(SymbolicSort::View, bytes.symbolic.node)?;
            for value in &bytes.values {
                self.value(value)?;
            }
            for cell in &bytes.cells {
                self.pool(PoolReferenceKind::ByteValue, cell.value, bytes.values.len())?;
            }
        }
        for account in &self.report.accounts {
            self.account(account)?;
        }
        for store in &self.report.stores {
            self.root(SymbolicSort::Slots, store.storage_root.node)?;
            self.root(SymbolicSort::Slots, store.transient_root.node)?;
            for fact in store.storage_facts.iter().chain(&store.transient_facts) {
                self.root(SymbolicSort::Word, fact.expression.node)?;
                self.value(&fact.value)?;
            }
            for value in [
                &store.storage_default,
                &store.transient_default,
                &store.balance_default,
                &store.nonce_default,
            ] {
                self.value(value)?;
            }
            for account in &store.accounts {
                self.account(account)?;
            }
            for log in &store.logs {
                self.buffer(log.data)?;
                for topic in &log.topics {
                    self.value(topic)?;
                }
            }
        }
        for state in &self.report.states {
            self.machine(&state.entry)?;
            if let Some(exit) = &state.exit {
                self.machine(exit)?;
            }
        }
        for block in &self.report.cfg {
            self.program(block.program)?;
            self.source_instructions(block)?;
            if let Some(key) = &block.process {
                if block.basic_block.is_some()
                    || block.start_pc.is_some()
                    || !block.context.is_empty()
                    || block.code_address.is_some()
                    || block.storage_address.is_some()
                    || !block.entry_stack.is_empty()
                    || !block.exit_stack.is_empty()
                    || !block.executed_pcs.is_empty()
                {
                    return Err(ReportValidationFailure::Process(
                        crate::ProcessValidationKind::PrivateState,
                    ));
                }
                self.process_program(key, block.program)?;
                if self.report.ssa.blocks.iter().any(|ssa| {
                    ssa.state == block.id
                        && (!ssa.phis.is_empty()
                            || !ssa.instructions.is_empty()
                            || ssa.exit_frames.iter().any(|frame| !frame.is_empty()))
                }) {
                    return Err(ReportValidationFailure::Process(
                        crate::ProcessValidationKind::PrivateState,
                    ));
                }
            } else if block.basic_block.is_none()
                || block.code_address.is_none()
                || block.storage_address.is_none()
            {
                return Err(ReportValidationFailure::Process(
                    crate::ProcessValidationKind::PrivateState,
                ));
            }
        }
        for machine in &self.report.pending_machines {
            self.machine(machine)?;
        }
        for frontier in &self.report.frontiers {
            if let Some(payload) = frontier.payload {
                self.pending(payload)?;
            }
            if let Some(range) = &frontier.range {
                self.pending(range.payload)?;
                for value in [
                    &range.condition,
                    &range.request.offset,
                    &range.request.size,
                    &range.request.limit,
                ] {
                    self.value(value)?;
                }
            }
        }
        for outcome in &self.report.outcomes {
            self.buffer(outcome.data)?;
            self.store(outcome.store)?;
        }
        for (index, summary) in self.report.abstract_summaries.iter().enumerate() {
            if let Some(root) = summary.root_procedure {
                self.pool(
                    PoolReferenceKind::AbstractProcedure,
                    root,
                    summary.procedures.len(),
                )?;
            }
            if !self
                .report
                .states
                .iter()
                .any(|state| state.state == summary.source_state)
            {
                return Err(ReportValidationFailure::Pool(PoolReferenceFailure {
                    kind: PoolReferenceKind::MachineState,
                    index: summary.source_state,
                    length: self.report.states.len(),
                }));
            }
            for procedure in &summary.procedures {
                if let Some(key) = &procedure.process {
                    self.process_program(key, None)?;
                    if procedure.kind != key.source.kind()
                        || procedure.semantics != key.source.semantics()
                        || procedure.is_static != key.is_static
                        || procedure.code_address.is_some()
                        || procedure.code_hash.is_some()
                        || procedure.storage_address.is_some()
                        || !procedure.covered_blocks.is_empty()
                    {
                        return Err(ReportValidationFailure::Process(
                            crate::ProcessValidationKind::Identity,
                        ));
                    }
                    let input = procedure.process_input.as_ref().ok_or(
                        ReportValidationFailure::Process(crate::ProcessValidationKind::Operands),
                    )?;
                    for value in [&input.address, &input.caller, &input.value] {
                        self.value(value)?;
                    }
                } else if procedure.process_input.is_some() {
                    return Err(ReportValidationFailure::Process(
                        crate::ProcessValidationKind::Operands,
                    ));
                }
                for dependency in &procedure.dependencies {
                    self.pool(
                        PoolReferenceKind::AbstractProcedure,
                        *dependency,
                        summary.procedures.len(),
                    )?;
                }
            }
            self.regional(index, summary)?;
        }
        for value in &self.report.ssa.values {
            for fact in &value.facts {
                if let Some(value) = &fact.value {
                    self.value(value)?;
                }
            }
        }
        Ok(())
    }
}
impl AnalysisGraph {
    /// Check graph-local references against the publication's shared source directory.
    pub fn validate(&self, programs: &[crate::ProgramInfo]) -> Result<(), ReportValidationFailure> {
        let max_work = self.metadata.postprocessing_max_work;
        let work = self.metadata.postprocessing_work;
        if max_work == 0 || work > max_work {
            return Err(ReportValidationFailure::Postprocessing(
                crate::PostprocessingWorkFailure { max_work, work },
            ));
        }
        self.validate_source(programs)?;
        Validation {
            report: self,
            programs,
            graph: &self.symbolic,
        }
        .all()
    }
}
impl AnalysisReport {
    /// Check schema, graph placement and all namespace-qualified references.
    pub fn validate(&self) -> Result<(), ReportValidationFailure> {
        if self.schema_version != crate::SCHEMA_VERSION {
            return Err(ReportValidationFailure::Schema(self.schema_version));
        }
        if self.selected_graph().is_none() || (self.global.is_none() && self.native.is_none()) {
            return Err(ReportValidationFailure::Hybrid(
                crate::HybridValidationKind::Namespace,
            ));
        }
        self.validate_catalog()?;
        for graph in [&self.global, &self.native].into_iter().flatten() {
            graph.validate(&self.programs)?;
            if self.postprocessing_work > graph.metadata.postprocessing_max_work {
                return Err(ReportValidationFailure::Postprocessing(
                    crate::PostprocessingWorkFailure {
                        max_work: graph.metadata.postprocessing_max_work,
                        work: self.postprocessing_work,
                    },
                ));
            }
        }
        self.validate_hybrid()
    }
}
