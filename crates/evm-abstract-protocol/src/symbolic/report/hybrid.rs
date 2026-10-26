//! Validate delivered references and qualifications, without deriving closure.
use crate::{
    AnalysisNamespace, AnalysisReport, AnalysisStatus, HybridInterpretation, HybridLocalScope,
    HybridNativeBinding, HybridValidationKind as Kind, ReportValidationFailure,
};
mod rpc;

/// Public input qualification exists before rich machine/history projection.
struct GlobalInput<'a> {
    fork: crate::Fork,
    to: &'a crate::AddressValue,
    entry_address: &'a str,
    root_binding: &'a crate::RootInputBinding,
    snapshot: &'a crate::RootTerminalSnapshot,
    revision: crate::DecimalU64,
    max_work: u64,
}
impl GlobalInput<'_> {
    fn input_namespace(&self) -> Option<&str> {
        match self.root_binding {
            crate::RootInputBinding::Namespace(source) => Some(&source.coordinate),
            crate::RootInputBinding::Physical(_) => None,
        }
    }
}

fn invalid(kind: Kind) -> ReportValidationFailure {
    ReportValidationFailure::Hybrid(kind)
}

impl AnalysisReport {
    pub(super) fn validate_hybrid(&self) -> Result<(), ReportValidationFailure> {
        let Some(hybrid) = &self.hybrid else {
            let graph = self
                .native
                .as_ref()
                .ok_or_else(|| invalid(Kind::Namespace))?;
            if self.global.is_some()
                || self.selected_view != AnalysisNamespace::Native
                || self.status != graph.status
                || self.work != graph.metadata.work
                || self.transfers != graph.transfers
            {
                return Err(invalid(Kind::Coverage));
            }
            return Ok(());
        };
        let coverage = &hybrid.coverage;
        let precision = hybrid.native_coverage();
        if coverage.requested_detail_regions != precision.requested_detail_regions
            || [&self.global, &self.native]
                .into_iter()
                .flatten()
                .any(|graph| {
                    graph.metadata.limits.max_detail_regions != coverage.requested_detail_regions
                })
            || [coverage, precision].into_iter().any(|coverage| {
                coverage.detail_regions.len() as u64 > coverage.requested_detail_regions
                    || (coverage.requested_detail_regions == 0
                        && (coverage.detail_config.is_some()
                            || coverage.detail_revision.is_some()
                            || coverage.local_detail_sites != 0))
            })
        {
            return Err(invalid(Kind::Coverage));
        }
        let baseline = if let Some(graph) = self.global.as_ref() {
            GlobalInput {
                fork: graph.fork,
                to: &graph.metadata.environment.to,
                entry_address: &graph.metadata.entry_address,
                root_binding: &graph.metadata.root_binding,
                snapshot: &graph.metadata.source_snapshot,
                revision: graph.metadata.revision,
                max_work: graph.metadata.limits.max_work,
            }
        } else if let Some(minimum) = self.minimum.as_ref() {
            GlobalInput {
                fork: minimum.fork,
                to: &minimum.environment.to,
                entry_address: &minimum.entry_address,
                root_binding: &minimum.root_binding,
                snapshot: &minimum.source_snapshot,
                revision: minimum.revision,
                max_work: minimum.config.max_work,
            }
        } else {
            return Err(invalid(Kind::Namespace));
        };
        let complete_export = matches!(self.rich_export, crate::RichExportOutcome::Complete);
        if *baseline.snapshot != coverage.snapshot
            || baseline.revision != coverage.baseline_revision
            || self.native.as_ref().is_some_and(|graph| {
                graph.metadata.source_snapshot != precision.snapshot
                    || Some(graph.metadata.revision) != precision.detail_revision
            })
        {
            return Err(invalid(Kind::Source));
        }
        if self.status
            != if coverage.baseline_current {
                AnalysisStatus::Converged
            } else {
                AnalysisStatus::Incomplete
            }
            || ((complete_export || self.native.is_some())
                && precision.local_detail_sites != hybrid.local_prefixes.len())
            || (coverage.chosen == HybridInterpretation::ProcessWithDetail
                && (!coverage.baseline_current
                    || (complete_export && hybrid.local_prefixes.is_empty())))
            || (coverage.chosen == HybridInterpretation::Native
                && (!coverage.baseline_current
                    || !coverage.detail_current
                    || coverage.detail_binding != HybridNativeBinding::OriginalOwner))
        {
            return Err(invalid(Kind::Coverage));
        }
        for work in [&coverage.work, &precision.work] {
            if work
                .prior
                .checked_add(work.baseline)
                .and_then(|sum| sum.checked_add(work.preparation))
                .and_then(|sum| sum.checked_add(work.detail))
                != Some(work.total)
                || work
                    .preparation
                    .checked_add(work.detail)
                    .is_none_or(|sum| sum > work.detail_allowance)
                || work.total > baseline.max_work
            {
                return Err(invalid(Kind::Work));
            }
        }
        self.validate_rpc_hybrid(hybrid, baseline.max_work, complete_export)?;
        let native = self.graph(AnalysisNamespace::Native);
        if !coverage.baseline_current && native.is_some() {
            return Err(invalid(Kind::Coverage));
        }
        for region in &precision.detail_regions {
            let valid = region.source.fork() == baseline.fork
                && match &region.source {
                    crate::ProcessSource::Runtime(source) => {
                        region.code_address.as_ref() == Some(&source.source)
                            && region.code_hash == source.hash.to_string()
                    }
                    crate::ProcessSource::InputProgram(source) => {
                        region.code_address.is_none()
                            && region.code_hash == source.hash.to_string()
                            && baseline.input_namespace().is_some()
                    }
                    _ => false,
                };
            if !valid {
                return Err(invalid(Kind::Source));
            }
        }
        if coverage.baseline_config.root_terminal_coverage
            || precision.detail_revision.is_some() != precision.detail_config.is_some()
            || (complete_export && native.is_some() != precision.detail_revision.is_some())
        {
            return Err(invalid(Kind::Source));
        }
        // Qualification fields must agree with the ORIGINAL input as well as
        // with each other. A conditioned native copy has Concrete To, while
        // the immutable baseline deliberately retains its original Symbolic To.
        match &precision.detail_binding {
            HybridNativeBinding::OriginalOwner => {
                if !matches!(baseline.to, crate::AddressValue::Concrete(_))
                    || native.is_some_and(|native| &native.metadata.environment.to != baseline.to)
                {
                    return Err(invalid(Kind::Source));
                }
            }
            HybridNativeBinding::SymbolicOwnerNamespace(binding) => {
                if !matches!(baseline.to, crate::AddressValue::Symbolic(_))
                    || binding.namespace != baseline.entry_address
                    || native.is_some_and(|native| {
                        binding.namespace != native.metadata.entry_address
                            || native.metadata.environment.to
                                != crate::AddressValue::Concrete(binding.namespace.clone())
                    })
                {
                    return Err(invalid(Kind::Source));
                }
            }
        }
        for (index, prefix) in hybrid.local_prefixes.iter().enumerate() {
            let native = native.ok_or_else(|| invalid(Kind::Namespace))?;
            if prefix.namespace != AnalysisNamespace::Native
                || hybrid.local_prefixes[..index]
                    .iter()
                    .any(|other| other.state == prefix.state)
            {
                return Err(invalid(Kind::Namespace));
            }
            if prefix.snapshot != precision.snapshot
                || Some(prefix.revision) != precision.detail_revision
                || prefix.binding != precision.detail_binding
                || prefix.scope
                    != match &precision.detail_binding {
                        HybridNativeBinding::OriginalOwner => HybridLocalScope::RetainedInputPrefix,
                        HybridNativeBinding::SymbolicOwnerNamespace(_) => {
                            HybridLocalScope::NativeOwnerNamespacePrefix
                        }
                    }
            {
                return Err(invalid(Kind::Source));
            }
            let state = native
                .states
                .iter()
                .find(|state| state.state == prefix.state)
                .ok_or_else(|| invalid(Kind::Input))?;
            let frame = state
                .entry
                .frames
                .last()
                .ok_or_else(|| invalid(Kind::Input))?;
            let recorded = &prefix.frame;
            let selected = recorded.kind == crate::CodeKind::Runtime
                && precision.detail_regions.iter().any(|region| {
                    recorded.code_hash.as_ref() == Some(&region.code_hash)
                        && match &region.source {
                            crate::ProcessSource::Runtime(source) => {
                                recorded.code_address.as_ref() == Some(&source.source)
                                    && region.code_address.as_ref() == Some(&source.source)
                            }
                            crate::ProcessSource::InputProgram(_) => {
                                region.code_address.is_none()
                                    && recorded.code_address.as_ref()
                                        == Some(&native.metadata.entry_address)
                                    && matches!(
                                        precision.detail_binding,
                                        HybridNativeBinding::SymbolicOwnerNamespace(_)
                                    )
                            }
                            _ => false,
                        }
                });
            if recorded.process.is_some()
                || frame.process.is_some()
                || recorded.code_address != frame.code_address
                || recorded.code_hash != frame.code_hash
                || recorded.storage_address != frame.storage_address
                || recorded.address_value != frame.address_value
                || recorded.caller != frame.caller
                || recorded.kind != frame.kind
                || recorded.is_static != frame.is_static
                || recorded.basic_block != frame.basic_block
                || recorded.stack_height != Some(frame.stack.len())
                || recorded.context != frame.context
                || prefix.context_selected != selected
                || (!selected && !recorded.context.is_empty())
            {
                return Err(invalid(Kind::Input));
            }
            let block = native
                .cfg
                .iter()
                .find(|block| block.id == prefix.state)
                .ok_or_else(|| invalid(Kind::Receipt))?;
            if prefix.instructions.is_empty()
                || prefix
                    .instructions
                    .iter()
                    .map(|instruction| instruction.pc)
                    .ne(block.executed_pcs.iter().copied())
                || prefix.instructions.iter().any(|instruction| {
                    block
                        .source_block()
                        .and_then(|source| self.source_block(source))
                        .is_none_or(|source| {
                            !source
                                .instructions
                                .iter()
                                .any(|source| source.pc == instruction.pc)
                        })
                })
                || native
                    .ssa
                    .blocks
                    .iter()
                    .find(|block| block.state == prefix.state)
                    .is_none_or(|block| {
                        block.coverage != crate::BlockCoverage::Current
                            || prefix.instructions.iter().any(|instruction| {
                                !block.instructions.iter().any(|ssa| {
                                    ssa.pc == instruction.pc && ssa.progress == instruction.phase
                                })
                            })
                    })
                || prefix.successors.iter().any(|successor| {
                    !native.cfg.iter().any(|block| block.id == successor.state)
                        || !native.edges.iter().any(|edge| {
                            edge.from == prefix.state
                                && edge.to == successor.state
                                && edge.kind == successor.kind
                        })
                })
            {
                return Err(invalid(Kind::Receipt));
            }
            if prefix.frontiers.iter().copied().ne(native
                .frontiers
                .iter()
                .enumerate()
                .filter(|(_, frontier)| frontier.from == Some(prefix.state))
                .map(|(index, _)| index))
            {
                return Err(invalid(Kind::Frontier));
            }
        }
        Ok(())
    }
}
