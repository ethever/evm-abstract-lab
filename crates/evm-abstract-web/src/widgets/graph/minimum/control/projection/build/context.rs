use super::{ControlOrigin, GraphScope, ProjectionError};
use evm_abstract_protocol::{MinimumSummary, ProcessLocation, RegionalFrame, RegionalNode};

pub(super) fn resolve(
    summary: &MinimumSummary,
    scope: GraphScope,
) -> Result<(Vec<&RegionalNode>, Option<ControlOrigin>), ProjectionError> {
    let cfg = &summary.relation.regional_cfg;
    if matches!(scope, GraphScope::Overview { .. }) {
        return Ok((
            cfg.nodes.iter().collect(),
            cfg.nodes.first().map(|node| ControlOrigin {
                node: node.id,
                location: None,
            }),
        ));
    }
    let (procedure, requested) = match scope {
        GraphScope::Entry { .. } => (
            summary
                .relation
                .root_procedure
                .ok_or(ProjectionError::MissingContext)?,
            None,
        ),
        GraphScope::Context {
            procedure, anchor, ..
        } => (procedure, anchor),
        GraphScope::Overview { .. } => unreachable!(),
    };
    let context = summary
        .relation
        .procedures
        .get(procedure)
        .ok_or(ProjectionError::MissingContext)?;
    let belongs = |node: &RegionalNode| match (&context.process, &node.frame.process) {
        (Some(entry), Some(frame)) => entry.same_context(frame),
        (None, None) => {
            context.code_address == node.frame.code_address
                && context.code_hash == node.frame.code_hash
                && context.storage_address == node.frame.storage_address
                && context.is_static == node.frame.is_static
                && context.kind == node.frame.kind
        }
        _ => false,
    };
    let anchor = if let Some(requested) = requested {
        let node = cfg
            .nodes
            .iter()
            .find(|node| node.id == requested.node && node.procedure == procedure)
            .ok_or(ProjectionError::InvalidAnchor)?;
        if !belongs(node) || !admitted(node, requested.location) {
            return Err(ProjectionError::InvalidAnchor);
        }
        Some(node)
    } else {
        cfg.nodes
            .iter()
            .find(|node| {
                node.procedure == procedure
                    && match (&context.process, &node.frame.process) {
                        (Some(entry), Some(frame)) => entry == frame,
                        (None, None) => true,
                        _ => false,
                    }
            })
            .or_else(|| {
                cfg.nodes
                    .iter()
                    .find(|node| node.procedure == procedure && belongs(node))
            })
    };
    let members = cfg
        .nodes
        .iter()
        .filter(|node| {
            node.procedure == procedure
                && belongs(node)
                && anchor.is_none_or(|anchor| compatible(&anchor.frame, &node.frame))
        })
        .collect();
    let origin = requested.or_else(|| {
        anchor.map(|node| ControlOrigin {
            node: node.id,
            location: None,
        })
    });
    Ok((members, origin))
}

fn admitted(node: &RegionalNode, location: Option<ProcessLocation>) -> bool {
    location.is_none_or(|location| {
        node.frame
            .process
            .as_ref()
            .is_some_and(|frame| frame.location == location)
            || node
                .transfer
                .as_ref()
                .and_then(|transfer| transfer.process.as_ref())
                .and_then(|process| process.control.as_ref())
                .is_some_and(|control| {
                    control.through.contains(&location) || control.boundaries.contains(&location)
                })
    })
}

fn compatible(a: &RegionalFrame, b: &RegionalFrame) -> bool {
    let source = match (&a.process, &b.process) {
        (Some(a), Some(b)) => a.same_context(b),
        (None, None) => {
            a.code_address == b.code_address
                && a.code_hash == b.code_hash
                && a.storage_address == b.storage_address
                && a.kind == b.kind
                && a.is_static == b.is_static
        }
        _ => false,
    };
    source && a.caller == b.caller && a.address_value == b.address_value && a.context == b.context
}
