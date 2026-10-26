//! Pure source grouping and bounded scope projection. The renderer lays out only
//! these cards; directory presence alone never admits a regional receipt.

mod build;
mod window;

use super::scope::{
    DEFAULT_POINT_LIMIT, FlowOrigin, GraphProjection, GraphScope, ProjectionEdge, ProjectionError,
};
use evm_abstract_protocol::AnalysisReport;

impl GraphProjection {
    pub(crate) fn build(
        report: &AnalysisReport,
        scope: GraphScope,
    ) -> Result<Self, ProjectionError> {
        Self::with_limit(report, scope, DEFAULT_POINT_LIMIT)
    }

    pub(crate) fn with_limit(
        report: &AnalysisReport,
        scope: GraphScope,
        point_limit: usize,
    ) -> Result<Self, ProjectionError> {
        let projection = build::build(report, scope)?;
        if matches!(scope, GraphScope::Overview { .. }) {
            Ok(projection)
        } else {
            Ok(window::project(report, projection, point_limit.max(1)))
        }
    }
}

fn edge(
    edges: &mut Vec<ProjectionEdge>,
    from: usize,
    to: usize,
    kind: evm_abstract_protocol::EdgeKind,
    origin: FlowOrigin,
) {
    if let Some(edge) = edges
        .iter_mut()
        .find(|edge| edge.from == from && edge.to == to && edge.kind == kind)
    {
        if !edge.origins.contains(&origin) {
            edge.origins.push(origin);
        }
    } else {
        edges.push(ProjectionEdge {
            from,
            to,
            kind,
            origins: vec![origin],
        });
    }
}

#[cfg(test)]
mod tests;
