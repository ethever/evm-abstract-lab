//! Real instruction coverage survives a retained guard's encoding-work limit.

use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{
        self, DiagnosticKind, EdgeKind, ExecutionConfig, MachineEdgeKind, OutcomeKind, Status,
    },
    domain::{
        relational::{Constraint, QueryReason},
        symbolic::ExprKind,
    },
    world::{Account, Entry, World},
};

#[test]
fn copied_unknown_hash_branch_covers_both_retained_alternatives() {
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "guard-limit", "unknown calldata");
    // Copy 64 unknown calldata bytes, hash them and branch on the hash.
    // Both edges terminate normally; no concrete caller/content is supplied.
    let account = Account::from_hex("60405f5f3760405f20600d57005b00", Fork::Osaka).unwrap();
    world.insert(address, account).unwrap();
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000,
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.max_nodes = 64;
    let result = analysis::analyze_world(world, Entry::new(address), config).unwrap();
    assert_eq!(result.status(), Status::Converged);
    assert!(result.frontiers().is_empty());
    assert_eq!(
        result
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .count(),
        2,
        "both retained hash alternatives terminate normally"
    );
    let branches = result
        .edges()
        .iter()
        .filter_map(|edge| match edge.kind {
            MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue) => Some((edge.to, true)),
            MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse) => Some((edge.to, false)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(branches.len(), 2);
    let mut retained_guards = Vec::new();
    for (target, nonzero) in branches {
        let guard = result.states()[target]
            .entry
            .relations
            .constraints()
            .find_map(|c| match c {
                Constraint::Truth {
                    expression,
                    nonzero: retained,
                } if *retained == nonzero
                    && matches!(expression.kind(), ExprKind::Keccak { .. }) =>
                {
                    Some(expression.clone())
                }
                _ => None,
            })
            .expect("each branch retains the actual unknown-content hash condition");
        retained_guards.push(guard);
    }
    assert_eq!(retained_guards[0], retained_guards[1]);
    assert!(result.diagnostics().iter().any(|d| d.pc == 11
        && d.kind == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)));
}
