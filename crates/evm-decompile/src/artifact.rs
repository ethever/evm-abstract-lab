//! Transactional stage adoption and the shared work ledger.
use crate::budget::{count, work_budget};
use crate::{
    Artifact, ArtifactId, Cancellation, CloneRelation, Completion, Config, Diagnostic,
    DiagnosticCode, EdgeRedirect, EntityId, EntityRef, Input, Ir, Limits, Origin, Phase, Progress,
    SCHEMA_VERSION, Severity, SourceMappings, SourceSpan, Stage, StageDiff, StageId, StageKind,
    Statistics, StructuredKind, TransformEvent, Validation, ValidationCheck, ValidationStatus,
    ValueKind,
};
use evm_decompile_model::{Record, diff_ir, records};
use evm_resource::WorkBudget;
mod optional;
#[cfg(test)]
mod tests;
pub(crate) use optional::OptionalStop;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Builder<P: FnMut(Progress)> {
    observer: P,
    progress: Progress,
    last_reported_work: usize,
    pub artifact: Artifact,
    pub budget: WorkBudget,
    pub cancel: Cancellation,
    next_entity: u64,
    published_entities: BTreeMap<EntityId, StageId>,
    pub stopped: bool,
    optional: Option<optional::Allowance>,
}
/// Fully paid and verified publication. The owned IR and provenance cannot be
/// replaced after quotation, and adoption consumes this transaction once.
pub(crate) struct PreparedPublication {
    source: Option<StageId>,
    source_history: u64,
    source_stages: usize,
    units: usize,
    kind: StageKind,
    stage: Stage,
    event: TransformEvent,
    origins: Vec<Origin>,
    published: Vec<EntityId>,
    history_entities: u64,
    prior_phase: Phase,
}
impl PreparedPublication {
    pub fn ir(&self) -> &Ir {
        &self.stage.ir
    }
    pub fn passed(&self) -> bool {
        self.stage.validation.status == ValidationStatus::Passed
    }
    pub fn candidate_clone_work(&self) -> usize {
        self.origins
            .iter()
            .fold(self.units.saturating_mul(4), |work, origin| {
                work.saturating_add(origin.parents.len()).saturating_add(1)
            })
    }
    pub fn parents(&self) -> BTreeMap<EntityId, Vec<EntityId>> {
        self.origins
            .iter()
            .map(|origin| {
                (
                    origin.entity.entity,
                    origin.parents.iter().map(|parent| parent.entity).collect(),
                )
            })
            .collect()
    }
}

impl<P: FnMut(Progress)> Builder<P> {
    pub fn new(input: &Input, config: &Config, cancel: &Cancellation, observer: P) -> Self {
        let mut bytes = input.fork.to_string().into_bytes();
        bytes.push(0);
        bytes.extend_from_slice(&input.runtime_bytecode);
        bytes.extend_from_slice(format!("schema={SCHEMA_VERSION};config={config:?}").as_bytes());
        // A refused configuration owns an inert ledger and cannot execute pay().
        // Keep the requested configuration intact for identity and diagnostics.
        let (budget, refusal) = match work_budget(config.limits.max_work) {
            Ok(budget) => (budget, None),
            Err(error) => (WorkBudget::new(0), Some(error)),
        };
        let mut builder = Self {
            observer,
            progress: Progress::default(),
            last_reported_work: 0,
            artifact: Artifact {
                schema_version: SCHEMA_VERSION,
                id: ArtifactId(format!("{:x}", alloy_primitives::keccak256(bytes))),
                input: input.clone(),
                config: config.clone(),
                completion: Completion::Complete,
                diagnostics: Vec::new(),
                stages: Vec::new(),
                events: Vec::new(),
                mappings: SourceMappings::default(),
                current_stage: None,
                statistics: Statistics::default(),
                readability: Vec::new(),
            },
            budget,
            cancel: cancel.clone(),
            next_entity: 1,
            published_entities: BTreeMap::new(),
            stopped: false,
            optional: None,
        };
        if let Some(error) = refusal {
            builder.stop(
                Completion::Failed,
                DiagnosticCode::Limit,
                &error.to_string(),
            );
        }
        builder
    }
    pub fn notify_phase(&mut self, phase: Phase) {
        self.progress.phase = phase;
        self.progress.work = count(self.budget.used());
        self.progress.stage = self.artifact.current_stage;
        if let Some(stage) = self.artifact.current() {
            self.progress.blocks = self.progress.blocks.max(count(stage.ir.blocks.len()));
            self.progress.operations = self.progress.operations.max(
                stage
                    .ir
                    .blocks
                    .iter()
                    .map(|b| count(b.operations.len()))
                    .sum(),
            );
        }
        self.last_reported_work = self.budget.used();
        (self.observer)(self.progress.clone());
    }
    pub fn id(&mut self) -> EntityId {
        let Some(next) = self.next_entity.checked_add(1) else {
            self.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "artifact entity identity space exhausted",
            );
            return EntityId(0);
        };
        let id = EntityId(self.next_entity);
        self.next_entity = next;
        id
    }
    pub fn limits(&self) -> &Limits {
        &self.artifact.config.limits
    }
    pub fn pay(&mut self, work: usize) -> bool {
        if self.stopped {
            return false;
        }
        if self.cancel.is_cancelled() {
            self.stop(
                Completion::Cancelled,
                DiagnosticCode::Cancelled,
                "decompilation cancelled",
            );
            return false;
        }
        if let Some(local) = &mut self.optional
            && !local.admits(work)
        {
            return false;
        }
        if !self.budget.charge(work) {
            self.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "logical work limit exhausted",
            );
            return false;
        }
        if self.budget.used().saturating_sub(self.last_reported_work) >= 65_536 {
            self.notify_phase(self.progress.phase);
        }
        true
    }
    pub fn stop(&mut self, completion: Completion, code: DiagnosticCode, message: &str) {
        self.stopped = true;
        self.artifact.completion = completion;
        self.diagnostic(code, Severity::Warning, None, None, message.to_owned());
    }
    pub fn partial(&mut self) {
        if self.artifact.completion == Completion::Complete {
            self.artifact.completion = Completion::Partial;
        }
    }
    pub fn diagnostic(
        &mut self,
        code: DiagnosticCode,
        severity: Severity,
        entity: Option<EntityId>,
        source: Option<SourceSpan>,
        message: String,
    ) {
        if self
            .artifact
            .diagnostics
            .iter()
            .any(|d| d.code == code && d.entity == entity && d.message == message)
        {
            return;
        }
        self.artifact.diagnostics.push(Diagnostic {
            severity,
            code,
            stage: entity
                .map(|e| self.published_entities.get(&e).copied())
                .unwrap_or(self.artifact.current_stage),
            entity,
            source,
            message,
        });
    }
    pub fn current(&self) -> &Stage {
        self.artifact.current().expect("a validated source stage")
    }
    pub fn commit(
        &mut self,
        kind: StageKind,
        ir: Ir,
        explanation: (&str, &str),
        parents: BTreeMap<EntityId, Vec<EntityId>>,
        copies: Vec<(EntityId, EntityId)>,
        redirects: Vec<EdgeRedirect>,
    ) -> bool {
        let Some(publication) =
            self.prepare_publication(kind, ir, explanation, parents, copies, redirects)
        else {
            return false;
        };
        self.adopt_publication(publication)
    }
    pub fn prepare_publication(
        &mut self,
        kind: StageKind,
        ir: Ir,
        explanation: (&str, &str),
        parents: BTreeMap<EntityId, Vec<EntityId>>,
        copies: Vec<(EntityId, EntityId)>,
        redirects: Vec<EdgeRedirect>,
    ) -> Option<PreparedPublication> {
        let (rule, reason) = explanation;
        if !self.pay(0) {
            return None;
        }
        let Some(stage_number) = u32::try_from(self.artifact.stages.len()).ok() else {
            self.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "artifact stage identity space exhausted",
            );
            return None;
        };
        let units = crate::transform::ir_size(&ir);
        let old_units = self
            .artifact
            .current()
            .map(|s| crate::transform::ir_size(&s.ir))
            .unwrap_or(0);
        let retained = units.saturating_add(old_units).saturating_mul(8);
        let Some(history_entities) = self
            .artifact
            .statistics
            .history_entities
            .checked_add(count(retained))
            .filter(|total| *total <= self.limits().max_history_entities)
        else {
            if let Some(local) = &mut self.optional {
                local.refuse(OptionalStop::History {
                    required: self
                        .artifact
                        .statistics
                        .history_entities
                        .saturating_add(count(retained)),
                    limit: self.artifact.config.limits.max_history_entities,
                });
            } else {
                self.stop(
                    Completion::Limited,
                    DiagnosticCode::Limit,
                    "immutable stage/provenance history limit exceeded; prior validated stage retained",
                );
            }
            return None;
        };
        // Index construction/diffs/SSA checks plus edge grouping and bounded
        // ancestry payloads are quoted before preparing any publication.
        let work = retained.saturating_mul(8).saturating_add(
            ir.blocks
                .len()
                .saturating_mul(ir.edges.len())
                .saturating_mul(8),
        );
        if !self.pay(work) {
            return None;
        }
        let id = StageId(stage_number);
        let previous = self.artifact.current_stage;
        let mut stage = Stage {
            id,
            parent: previous,
            kind,
            ir,
            validation: Validation {
                status: ValidationStatus::NotRun,
                checks: Vec::new(),
            },
        };
        let prior_phase = self.progress.phase;
        self.notify_phase(Phase::Validate);
        stage.validation = crate::validate::validate_with_cancel(&stage, &self.cancel);
        let scope_matches = stage.ir.execution_domain == self.artifact.config.execution_domain
            && stage.ir.execution_fork
                == (!self.artifact.config.execution_domain.is_generic())
                    .then_some(self.artifact.input.fork);
        if !scope_matches {
            stage.validation.status = ValidationStatus::Failed;
        }
        stage.validation.checks.push(ValidationCheck {
            name: "frozen_execution_scope".to_owned(),
            passed: scope_matches,
            message: if scope_matches {
                "snapshot uses the frozen input and execution domain".to_owned()
            } else {
                "snapshot changes the frozen execution domain or its fork".to_owned()
            },
        });
        let source_matches = stage
            .ir
            .runtime_bytecode
            .as_ref()
            .is_none_or(|code| *code == self.artifact.input.runtime_bytecode);
        if !source_matches {
            stage.validation.status = ValidationStatus::Failed;
        }
        stage.validation.checks.push(ValidationCheck {
            name: "frozen_runtime_source".to_owned(),
            passed: source_matches,
            message: if source_matches {
                "snapshot retains the captured runtime source".to_owned()
            } else {
                "snapshot runtime source differs from the frozen input".to_owned()
            },
        });
        if !self.pay(0) {
            return None;
        }
        self.notify_phase(Phase::Provenance);
        if !self.pay(0) {
            return None;
        }
        let old = previous.and_then(|p| self.artifact.stage(p));
        if let Some(old) = old {
            let check = crate::validate::validate_transition(old, &stage, &copies, &self.cancel);
            if !check.passed {
                stage.validation.status = ValidationStatus::Failed;
            }
            stage.validation.checks.push(check);
        }
        let changes = old
            .map(|s| diff_ir(&s.ir, &stage.ir))
            .unwrap_or_else(|| StageDiff {
                added: records(&stage.ir).keys().copied().collect(),
                ..StageDiff::default()
            });
        let changed: BTreeSet<_> = changes.changed.iter().copied().collect();
        let old_ids: BTreeSet<_> = old
            .map(|s| records(&s.ir).keys().copied().collect())
            .unwrap_or_default();
        let all = records(&stage.ir);
        let event_id = self.artifact.events.len() as u64;
        let prior_sources: BTreeMap<_, _> = self
            .artifact
            .mappings
            .origins
            .iter()
            .filter(|o| Some(o.entity.stage) == previous)
            .map(|o| (o.entity, o.source.clone()))
            .collect();
        let mut direct = BTreeMap::<EntityId, Vec<SourceSpan>>::new();
        let mut producer_index = BTreeMap::new();
        for b in &stage.ir.blocks {
            direct.insert(b.id, b.source.into_iter().collect());
            for o in &b.operations {
                let source: Vec<_> = o.source.into_iter().collect();
                direct.insert(o.id, source.clone());
                for v in o
                    .outputs
                    .iter()
                    .copied()
                    .chain(o.memory_output)
                    .chain(o.effect_output)
                {
                    direct.insert(v, source.clone());
                    producer_index.insert(v, o.id);
                }
            }
        }
        let mut origins = Vec::new();
        let ancestry: BTreeMap<_, _> = all
            .iter()
            .map(|(entity, record)| {
                let mut ids = inferred_parents(record);
                ids.extend(parents.get(entity).into_iter().flatten().copied());
                (*entity, ids)
            })
            .collect();
        for (entity, record) in &all {
            if self.cancel.is_cancelled() {
                self.stop(
                    Completion::Cancelled,
                    DiagnosticCode::Cancelled,
                    "cancelled while preparing provenance; prior validated stage retained",
                );
                return None;
            }
            let mut parent_ids = if old_ids.contains(entity) {
                vec![*entity]
            } else {
                inferred_parents(record)
            };
            parent_ids.extend(parents.get(entity).into_iter().flatten().copied());
            parent_ids.sort();
            parent_ids.dedup();
            let mut refs = Vec::new();
            if let Some(stage) = previous {
                // Newly constructed context parameters/edges can refer to a
                // candidate context block, itself copied from an old block.
                // Resolve that transformation ancestry to the prior snapshot;
                // semantic phi/backedge dependencies are deliberately absent.
                let mut seen = BTreeSet::new();
                let mut resolved = BTreeSet::new();
                while let Some(parent) = parent_ids.pop() {
                    if !self.pay(1) {
                        return None;
                    }
                    if old_ids.contains(&parent) {
                        resolved.insert(parent);
                    } else if seen.insert(parent)
                        && let Some(ids) = ancestry.get(&parent)
                    {
                        parent_ids.extend_from_slice(ids);
                    }
                }
                refs.extend(
                    resolved
                        .into_iter()
                        .map(|entity| EntityRef { stage, entity }),
                );
            }
            // Initial construction edges are a DAG: blocks precede parameters,
            // operations precede their results. Materialize producer ancestry.
            if previous.is_none() {
                let producers: Vec<_> = match record {
                    Record::Value(v) => producer_index
                        .get(&v.id)
                        .map(|id| vec![*id])
                        .unwrap_or_else(|| inferred_parents(record)),
                    _ => Vec::new(),
                };
                refs = producers
                    .into_iter()
                    .filter(|p| all.contains_key(p))
                    .map(|entity| EntityRef { stage: id, entity })
                    .collect();
            }
            let mut source = direct.get(entity).cloned().unwrap_or_default();
            for parent in &refs {
                // Keep structural links for inspection without copying an
                // entire shared failure/pass/containing tree's spans into
                // every guard. Predicate, block and edge spans anchor its row;
                // the referenced raw nodes retain their own complete origins.
                if matches!(
                    record,
                    Record::SharedProcedure(_) | Record::SharedInvocation(_)
                ) {
                    // Retain one real source anchor per body/site. The checked
                    // source map keeps complete navigation without copying a
                    // whole subtree span list into every invocation's history.
                    if let Some(span) = prior_sources
                        .get(parent)
                        .or_else(|| direct.get(&parent.entity))
                        .and_then(|s| s.first())
                    {
                        source.push(*span);
                    }
                    continue;
                }
                if let Record::LiftedControl(control) = record
                    && let Some(guard) = &control.guard
                    && (parent.entity == control.node
                        || parent.entity == guard.pass_node
                        || parent.entity == guard.failure_node
                        || guard
                            .continuation
                            .as_ref()
                            .is_some_and(|c| c.sequence == parent.entity))
                {
                    continue;
                }
                if let Some(s) = prior_sources
                    .get(parent)
                    .or_else(|| direct.get(&parent.entity))
                {
                    source.extend_from_slice(s);
                }
            }
            source.sort();
            source.dedup();
            origins.push(Origin {
                entity: EntityRef {
                    stage: id,
                    entity: *entity,
                },
                source,
                parents: refs,
                introduced_by: event_id,
                reason: if old_ids.contains(entity) && !changed.contains(entity) {
                    "preserved entity".to_owned()
                } else {
                    reason.to_owned()
                },
            });
        }
        if !self.pay(0) {
            return None;
        }
        let clones: Vec<_> = copies
            .into_iter()
            .filter_map(|(original, copy)| {
                // Faulting recovery copies discard their unexecuted suffix. Those
                // unpublished temporary identities are not stage occurrences.
                if !old_ids.contains(&original) || !all.contains_key(&copy) {
                    return None;
                }
                Some(CloneRelation {
                    original: EntityRef {
                        stage: previous?,
                        entity: original,
                    },
                    copy: EntityRef {
                        stage: id,
                        entity: copy,
                    },
                })
            })
            .collect();
        let links_passed = origins.iter().all(|o| {
            o.entity.stage == id
                && all.contains_key(&o.entity.entity)
                && o.parents.iter().all(|p| {
                    if p.stage == id {
                        all.contains_key(&p.entity)
                    } else {
                        Some(p.stage) == previous && old_ids.contains(&p.entity)
                    }
                })
        }) && redirects.iter().all(|r| {
            old_ids.contains(&r.before)
                && all.contains_key(&r.after)
                && matches!(all.get(&r.edge), Some(Record::Edge(e)) if e.to == r.after)
        });
        stage.validation.checks.push(ValidationCheck {
            name: "transformation_occurrence_references".to_owned(),
            passed: links_passed,
            message: if links_passed {
                "checked origins, retained clone endpoints and edge redirection occurrences"
            } else {
                "transformation metadata refers to a missing stage occurrence"
            }
            .to_owned(),
        });
        if !links_passed {
            stage.validation.status = ValidationStatus::Failed;
        }
        let validation = stage.validation.clone();
        let event = TransformEvent {
            id: event_id,
            from_stage: previous,
            to_stage: id,
            rule: rule.to_owned(),
            reason: reason.to_owned(),
            inputs: previous
                .map(|stage| {
                    old_ids
                        .iter()
                        .map(|entity| EntityRef {
                            stage,
                            entity: *entity,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            outputs: all
                .keys()
                .map(|entity| EntityRef {
                    stage: id,
                    entity: *entity,
                })
                .collect(),
            created: changes.added,
            deleted: changes.removed,
            clones,
            redirects,
            validation: validation.clone(),
        };
        self.notify_phase(prior_phase);
        Some(PreparedPublication {
            source: previous,
            source_history: self.artifact.statistics.history_entities,
            source_stages: self.artifact.stages.len(),
            units,
            kind,
            published: all.keys().copied().collect(),
            stage,
            event,
            origins,
            history_entities,
            prior_phase,
        })
    }

    pub fn adopt_publication(&mut self, publication: PreparedPublication) -> bool {
        if !self.pay(0) {
            return false;
        }
        if self.artifact.current_stage != publication.source
            || self.artifact.statistics.history_entities != publication.source_history
            || self.artifact.stages.len() != publication.source_stages
            || self.artifact.events.len() as u64 != publication.event.id
            || crate::transform::ir_size(&publication.stage.ir) != publication.units
            || publication.stage.parent != publication.source
            || publication.stage.kind != publication.kind
        {
            self.stop(
                Completion::Failed,
                DiagnosticCode::Validation,
                "prepared publication no longer matches its paid source/IR witness",
            );
            return false;
        }
        let PreparedPublication {
            stage,
            event,
            origins,
            published,
            history_entities,
            prior_phase,
            ..
        } = publication;
        let id = stage.id;
        let rule = event.rule.clone();
        let passed = stage.validation.status == ValidationStatus::Passed;
        // One publication point: cancellation/refusal cannot leave half an event,
        // origin table or stage attached to the artifact.
        self.artifact.statistics.history_entities = history_entities;
        for diagnostic in &mut self.artifact.diagnostics {
            if diagnostic.stage.is_none()
                && diagnostic
                    .entity
                    .is_some_and(|e| published.binary_search(&e).is_ok())
            {
                diagnostic.stage = Some(id);
            }
        }
        self.published_entities
            .extend(published.iter().map(|entity| (*entity, id)));
        self.artifact.mappings.origins.extend(origins);
        self.artifact.events.push(event);
        self.artifact.stages.push(stage);
        if passed {
            self.artifact.current_stage = Some(id);
        } else {
            self.partial();
            self.diagnostic(
                DiagnosticCode::Validation,
                Severity::Error,
                None,
                None,
                format!("{rule} candidate failed validation; prior stage retained"),
            );
        }
        self.notify_phase(prior_phase);
        passed
    }
    pub fn finish(mut self) -> Artifact {
        // Refused candidates have no retained entity occurrence. Their source
        // span remains a valid location without a dangling stage/entity link.
        for diagnostic in &mut self.artifact.diagnostics {
            if diagnostic
                .entity
                .is_some_and(|e| !self.published_entities.contains_key(&e))
            {
                diagnostic.entity = None;
            }
        }
        self.artifact.statistics.work = count(self.budget.used());
        if let Some(s) = self.artifact.current() {
            let (blocks, operations, values, edges) = (
                count(s.ir.blocks.len()),
                s.ir.blocks.iter().map(|b| count(b.operations.len())).sum(),
                count(s.ir.values.len()),
                count(s.ir.edges.len()),
            );
            self.artifact.statistics.blocks = blocks;
            self.artifact.statistics.operations = operations;
            self.artifact.statistics.values = values;
            self.artifact.statistics.edges = edges;
        }
        self.artifact
    }
}
impl<P: FnMut(Progress)> analysis_kernel::Meter for Builder<P> {
    type Stop = Completion;
    fn reserve(&mut self, work: usize) -> Result<(), Self::Stop> {
        if self.pay(work) {
            Ok(())
        } else {
            Err(self.artifact.completion)
        }
    }
    fn checkpoint(&self) -> Result<(), Self::Stop> {
        if self.cancel.is_cancelled() {
            Err(Completion::Cancelled)
        } else if self.stopped {
            Err(self.artifact.completion)
        } else {
            Ok(())
        }
    }
}
fn inferred_parents(record: &Record<'_>) -> Vec<EntityId> {
    match record {
        Record::Value(v) => match v.kind {
            ValueKind::Operation { operation, .. }
            | ValueKind::Effect {
                operation: Some(operation),
            }
            | ValueKind::Memory {
                operation: Some(operation),
            } => vec![operation],
            ValueKind::Parameter { block, .. }
            | ValueKind::MemoryParameter { block }
            | ValueKind::EffectParameter { block } => {
                vec![block]
            }
            _ => Vec::new(),
        },
        Record::Edge(e) => vec![e.from, e.to],
        Record::Node(n) => match &n.kind {
            StructuredKind::Block { block }
            | StructuredKind::Exit { block }
            | StructuredKind::PartialExit { block }
            | StructuredKind::Indirect { block, .. } => vec![*block],
            StructuredKind::Transfer { edge } => vec![*edge],
            StructuredKind::If { condition, .. } => vec![*condition],
            StructuredKind::Dispatcher { cases, .. } => cases.iter().map(|c| c.block).collect(),
            _ => Vec::new(),
        },
        Record::Transition(t) => vec![t.edge],
        Record::LiftedExpression(e) => match &e.kind {
            crate::LiftedExpressionKind::Value(value) => vec![*value],
            _ => Vec::new(),
        },
        Record::LiftedBlock(b) => vec![b.block],
        Record::SharedProcedure(p) => vec![p.body],
        Record::SharedInvocation(i) => vec![i.source],
        Record::LiftedRevert(r) => std::iter::once(r.block)
            .chain([r.operation, r.memory, r.offset, r.size])
            .chain(r.writes.iter().copied())
            .collect(),
        Record::LiftedConstruction(c) => vec![c.block],
        Record::LiftedStatement(s) => vec![s.operation],
        Record::LiftedTransfer(t) => vec![t.edge],
        Record::LiftedControl(c) => std::iter::once(c.node)
            .chain(c.guard.iter().flat_map(|g| g.source_references()))
            .collect(),
        _ => Vec::new(),
    }
}
