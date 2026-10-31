//! Qualified presentation state over the core-owned immutable artifact. This
//! module indexes transport identities, never infers transforms from geometry.

use evm_abstract_protocol::{
    JobId, PublicationVersion,
    decompile_model::{
        Artifact, ArtifactId, EntityId, EntityRef, StageId, StageKind, ValidationStatus,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Publication {
    pub job: JobId,
    pub version: PublicationVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub publication: Publication,
    pub artifact: ArtifactId,
    pub at: EntityRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntityKind {
    Block,
    Operation,
    Value,
    Edge,
    Structure,
    Transition,
}

#[derive(Default)]
pub(super) struct Index {
    pub entities: BTreeMap<EntityRef, EntityKind>,
    origins: BTreeMap<EntityRef, usize>,
}

impl Index {
    pub fn new(artifact: &Artifact) -> Self {
        let origins = artifact
            .mappings
            .origins
            .iter()
            .enumerate()
            .map(|(position, origin)| (origin.entity, position))
            .collect();
        let mut index = Self {
            origins,
            ..Self::default()
        };
        for stage in &artifact.stages {
            let mut add = |entity, kind| {
                index.entities.insert(
                    EntityRef {
                        stage: stage.id,
                        entity,
                    },
                    kind,
                );
            };
            for block in &stage.ir.blocks {
                add(block.id, EntityKind::Block);
                for operation in &block.operations {
                    add(operation.id, EntityKind::Operation);
                }
            }
            for value in &stage.ir.values {
                add(value.id, EntityKind::Value);
            }
            for edge in &stage.ir.edges {
                add(edge.id, EntityKind::Edge);
            }
            if let Some(program) = &stage.ir.structured {
                super::structured::visit(&program.root, &mut |node| {
                    add(node.id, EntityKind::Structure);
                    if let evm_abstract_protocol::decompile_model::StructuredKind::Dispatcher {
                        cases,
                        ..
                    } = &node.kind
                    {
                        for case in cases {
                            for transition in &case.transitions {
                                add(transition.id, EntityKind::Transition);
                            }
                        }
                    }
                });
            }
        }
        index
    }

    pub fn contains(&self, at: EntityRef) -> bool {
        self.entities.contains_key(&at)
    }

    pub fn origin<'a>(
        &self,
        artifact: &'a Artifact,
        at: EntityRef,
    ) -> Option<&'a evm_abstract_protocol::decompile_model::Origin> {
        artifact.mappings.origins.get(*self.origins.get(&at)?)
    }
}

#[derive(Default)]
pub(super) struct ViewState {
    pub publication: Option<Publication>,
    pub current: Option<StageId>,
    pub before: Option<StageId>,
    pub selected: Option<Selection>,
    pub source_pc: Option<usize>,
    pub event: Option<u64>,
    pub index: Index,
    pub source_navigation: bool,
    pub entity_navigation: bool,
    pub compare_navigation: bool,
    source_descendants: Option<(usize, Rc<Vec<EntityRef>>)>,
    entity_descendants: Option<(EntityRef, Rc<Vec<EntityRef>>)>,
    history: Vec<Selection>,
    cursor: usize,
}

impl ViewState {
    pub fn install(&mut self, artifact: &Artifact, publication: Publication) {
        self.index = Index::new(artifact);
        self.publication = Some(publication);
        self.current = artifact
            .current_stage
            .or_else(|| artifact.stages.last().map(|stage| stage.id));
        self.before = artifact
            .stages
            .iter()
            .find(|stage| stage.kind == StageKind::Baseline)
            .map(|stage| stage.id)
            .or_else(|| artifact.stages.first().map(|stage| stage.id));
        self.selected = None;
        self.source_pc = None;
        self.event = None;
        self.source_navigation = false;
        self.entity_navigation = false;
        self.compare_navigation = false;
        self.source_descendants = None;
        self.entity_descendants = None;
        self.history.clear();
        self.cursor = 0;
        if let Some(stage) = self.current.and_then(|id| artifact.stage(id))
            && let Some(entity) = stage
                .ir
                .entry
                .or_else(|| stage.ir.blocks.first().map(|block| block.id))
        {
            self.select(
                artifact,
                EntityRef {
                    stage: stage.id,
                    entity,
                },
            );
        }
    }

    pub fn select(&mut self, artifact: &Artifact, at: EntityRef) -> bool {
        if !self.index.contains(at) {
            return false;
        }
        let Some(publication) = self.publication else {
            return false;
        };
        let selected = Selection {
            publication,
            artifact: artifact.id.clone(),
            at,
        };
        if self.selected.as_ref() == Some(&selected) {
            return true;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.cursor + 1);
        }
        self.history.push(selected.clone());
        self.cursor = self.history.len() - 1;
        self.source_pc = self
            .index
            .origin(artifact, at)
            .and_then(|origin| origin.source.first())
            .map(|source| source.pc);
        self.event = self
            .index
            .origin(artifact, at)
            .map(|origin| origin.introduced_by);
        if let Some(pc) = self.source_pc {
            self.source_descendants(artifact, pc);
        }
        self.selected = Some(selected);
        true
    }

    pub fn navigate(&mut self, artifact: &Artifact, at: EntityRef) -> bool {
        if !self.select(artifact, at) {
            return false;
        }
        self.set_stage(artifact, at.stage);
        self.entity_navigation = true;
        true
    }

    pub fn set_stage(&mut self, artifact: &Artifact, id: StageId) {
        let Some(stage) = artifact.stage(id) else {
            return;
        };
        self.current = Some(id);
        let mut ancestors = BTreeSet::from([id]);
        let mut cursor = stage.parent;
        while let Some(parent) = cursor {
            if !ancestors.insert(parent) {
                break;
            }
            cursor = artifact.stage(parent).and_then(|stage| stage.parent);
        }
        if self
            .before
            .is_none_or(|before| !ancestors.contains(&before))
        {
            self.before = stage.parent.or(Some(stage.id));
        }
    }

    pub fn can_back(&self) -> bool {
        !self.history.is_empty() && self.cursor > 0
    }
    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    pub fn history(&mut self, artifact: &Artifact, forward: bool) {
        if forward && self.can_forward() {
            self.cursor += 1;
        } else if !forward && self.can_back() {
            self.cursor -= 1;
        } else {
            return;
        }
        let selection = self.history[self.cursor].clone();
        if selection.artifact == artifact.id
            && self.publication == Some(selection.publication)
            && self.index.contains(selection.at)
        {
            self.set_stage(artifact, selection.at.stage);
            self.source_pc = self
                .index
                .origin(artifact, selection.at)
                .and_then(|origin| origin.source.first())
                .map(|source| source.pc);
            self.event = self
                .index
                .origin(artifact, selection.at)
                .map(|origin| origin.introduced_by);
            self.selected = Some(selection);
            if let Some(pc) = self.source_pc {
                self.source_descendants(artifact, pc);
            }
            self.entity_navigation = true;
        }
    }

    pub fn selected_in(&self, artifact: &Artifact, stage: StageId) -> Option<EntityId> {
        self.selected
            .as_ref()
            .filter(|selected| {
                selected.artifact == artifact.id
                    && self.publication == Some(selected.publication)
                    && selected.at.stage == stage
            })
            .map(|selected| selected.at.entity)
    }

    pub fn select_source(&mut self, artifact: &Artifact, pc: usize) {
        self.source_pc = Some(pc);
        self.selected = None;
        self.source_descendants(artifact, pc);
    }

    pub fn source_descendants(&mut self, artifact: &Artifact, pc: usize) -> Rc<Vec<EntityRef>> {
        if self
            .source_descendants
            .as_ref()
            .is_none_or(|(old, _)| *old != pc)
        {
            self.source_descendants = Some((pc, Rc::new(artifact.descendants(pc))));
        }
        Rc::clone(
            &self
                .source_descendants
                .as_ref()
                .expect("source descendants prepared")
                .1,
        )
    }

    pub fn entity_descendants(&mut self, artifact: &Artifact, at: EntityRef) -> Rc<Vec<EntityRef>> {
        if self
            .entity_descendants
            .as_ref()
            .is_none_or(|(old, _)| *old != at)
        {
            self.entity_descendants = Some((at, Rc::new(artifact.derived_from(at))));
        }
        Rc::clone(
            &self
                .entity_descendants
                .as_ref()
                .expect("entity descendants prepared")
                .1,
        )
    }

    pub fn source_descendant_count(&self, pc: usize) -> Option<usize> {
        self.source_descendants
            .as_ref()
            .filter(|(old, _)| *old == pc)
            .map(|(_, rows)| rows.len())
    }
}

pub(super) fn check_artifact(artifact: &Artifact) -> Result<(), String> {
    if artifact.schema_version != evm_abstract_protocol::decompile_model::SCHEMA_VERSION {
        return Err(format!(
            "unsupported artifact schema {}",
            artifact.schema_version
        ));
    }
    let mut stages = BTreeSet::new();
    for stage in &artifact.stages {
        if !stages.insert(stage.id) {
            return Err(format!("duplicate stage {:?}", stage.id));
        }
        if let Some(parent) = stage.parent
            && (parent == stage.id || !stages.contains(&parent))
        {
            return Err(format!("missing preceding parent {parent:?}"));
        }
        let mut ids = BTreeSet::new();
        let mut duplicate = None;
        let mut check = |id| {
            if !ids.insert(id) {
                duplicate = Some(id);
            }
        };
        for block in &stage.ir.blocks {
            check(block.id);
            for operation in &block.operations {
                check(operation.id);
            }
        }
        for value in &stage.ir.values {
            check(value.id);
        }
        for edge in &stage.ir.edges {
            check(edge.id);
        }
        if let Some(program) = &stage.ir.structured {
            super::structured::visit(&program.root, &mut |node| {
                check(node.id);
                if let evm_abstract_protocol::decompile_model::StructuredKind::Dispatcher {
                    cases,
                    ..
                } = &node.kind
                {
                    for case in cases {
                        for transition in &case.transitions {
                            check(transition.id);
                        }
                    }
                }
            });
        }
        if let Some(id) = duplicate
            && stage.validation.status != ValidationStatus::Failed
        {
            return Err(format!("duplicate entity {id:?} in stage {:?}", stage.id));
        }
        let blocks: BTreeSet<_> = stage.ir.blocks.iter().map(|block| block.id).collect();
        if stage.validation.status != ValidationStatus::Failed
            && (stage.ir.entry.is_some_and(|entry| !blocks.contains(&entry))
                || stage
                    .ir
                    .edges
                    .iter()
                    .any(|edge| !blocks.contains(&edge.from) || !blocks.contains(&edge.to)))
        {
            return Err(format!("dangling graph endpoint in stage {:?}", stage.id));
        }
    }
    if artifact
        .current_stage
        .is_some_and(|id| !stages.contains(&id))
    {
        return Err("current stage does not exist".into());
    }
    if artifact
        .current()
        .is_some_and(|stage| stage.validation.status != ValidationStatus::Passed)
    {
        return Err("adopted current stage did not pass its checks".into());
    }
    let bytecode_length = artifact.input.runtime_bytecode.len();
    let mut origins = BTreeSet::new();
    for origin in &artifact.mappings.origins {
        if !origins.insert(origin.entity) {
            return Err("duplicate origin occurrence".into());
        }
        if origin
            .source
            .iter()
            .any(|source| source.pc >= source.end_pc || source.end_pc > bytecode_length)
        {
            return Err("source origin outside original bytecode".into());
        }
    }
    Ok(())
}
