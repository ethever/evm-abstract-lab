//! Every graph and pool reference is checked before a consumer follows it.
use crate::{
    AccountState, AnalysisReport, MachineSnapshot, PoolReferenceFailure, PoolReferenceKind,
    ReportValidationFailure, SymbolicGraph, SymbolicReference, SymbolicSort, ValueInfo,
};

struct Validation<'a> {
    report: &'a AnalysisReport,
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
            if !self.report.programs.iter().any(|program| program.id == id) {
                return Err(ReportValidationFailure::Pool(PoolReferenceFailure {
                    kind: PoolReferenceKind::Program,
                    index: id,
                    length: self.report.programs.len(),
                }));
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
            self.program(frame.program)?;
            self.store(frame.rollback_store)?;
            for id in [frame.memory, frame.calldata, frame.returndata] {
                self.buffer(id)?;
            }
            self.value(&frame.call_value)?;
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
        let env = &self.report.metadata.environment;
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
            for fact in store.storage_facts.iter().chain(&store.transient_facts) {
                self.root(SymbolicSort::Word, fact.expression.node)?;
                self.value(&fact.value)?;
            }
            for value in [
                &store.storage_default,
                &store.transient_default,
                &store.balance_default,
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
impl AnalysisReport {
    /// Validate the version and every symbolic, program, buffer/store/pending reference.
    pub fn validate(&self) -> Result<(), ReportValidationFailure> {
        if self.schema_version != crate::SCHEMA_VERSION {
            return Err(ReportValidationFailure::Schema(self.schema_version));
        }
        Validation {
            report: self,
            graph: &self.symbolic,
        }
        .all()
    }
}
