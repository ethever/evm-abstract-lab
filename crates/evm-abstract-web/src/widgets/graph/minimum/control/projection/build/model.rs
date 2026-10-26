//! Regional composition/admission evidence is retained beside source adjacency.
//! None of these records becomes a source edge merely because it names two PCs.

use super::{
    Builder, Destination, FlowOrigin, ModelRelation, TargetEntry, TargetSet, TargetSetKind,
};
use evm_abstract_protocol::{EdgeKind, RegionalNode};

impl Builder<'_> {
    pub(super) fn retain_relation(
        &mut self,
        from: &RegionalNode,
        to: &RegionalNode,
        kind: Option<EdgeKind>,
        origin: FlowOrigin,
    ) {
        let relation = ModelRelation {
            from: self.target(from, None),
            to: self.target(to, None),
            kind,
            origin,
        };
        if !self.projection.model_relations.contains(&relation) {
            self.projection.model_relations.push(relation);
        }
    }

    pub(super) fn finish_model(&mut self) {
        self.projection.counts.retained_topology = self
            .projection
            .model_relations
            .iter()
            .filter(|relation| relation.kind.is_none())
            .count();
        self.projection.counts.summarized_model_relations = self
            .projection
            .model_relations
            .iter()
            .filter(|relation| {
                !self.projection.edges.iter().any(|edge| {
                    relation.kind == Some(edge.kind) && edge.origins.contains(&relation.origin)
                })
            })
            .count();
        if self.projection.model_relations.is_empty() {
            return;
        }
        self.projection.model_set = Some(self.projection.target_sets.len());
        self.projection.target_sets.push(TargetSet {
            kind: TargetSetKind::ModelRelations,
            entries: self
                .projection
                .model_relations
                .iter()
                .flat_map(|relation| {
                    [relation.from, relation.to]
                        .into_iter()
                        .map(move |target| TargetEntry {
                            destination: Destination::Control(target),
                            via: relation.origin,
                        })
                })
                .collect(),
        });
    }
}
