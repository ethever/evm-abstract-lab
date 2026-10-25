use super::super::testing::{Table, execute_inner, initial};
use super::super::{Counters, EvmCallStack, EvmMachinePayload, EvmRootFrame, StorageReadPolicy};
use crate::{
    analysis::{
        ExecutionConfig, MachineEdgeKind, MachineError, PolicyContext, control::Control,
        machine::fixtures::fixture, worklist::runtime::TaskKind,
    },
    domain::{ProductAlgebra, ValueAlgebra, relational::QueryReason},
    resource::WorkBudget,
};

#[test]
fn refused_or_cancelled_self_delegate_admission_preserves_the_existing_root() {
    for cancelled in [false, true] {
        // An actual self-DELEGATECALL inherits source/owner/caller/value/static
        // but receives private calldata rather than the original environment.
        let (world, entry) = fixture(&[(0x101, "60016000555f5f5f5f6101015af400")]);
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let domain = algebra.borrowed();
        let mut config = ExecutionConfig::default();
        if !cancelled {
            config.analysis.max_states = 1;
        }
        let mut budget = WorkBudget::new(config.max_work);
        let root = initial(&world, &entry, domain).unwrap();
        let execution = execute_inner(
            &world,
            &entry,
            root.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        let call = execution
            .successors
            .into_iter()
            .find(|successor| successor.kind == MachineEdgeKind::Call)
            .unwrap()
            .payload;
        assert_eq!(root.active().key, call.active().key);
        assert!(root.active().environment_calldata);
        assert!(!call.active().environment_calldata);
        call.pay_clone_with(&mut budget).unwrap();
        let child = EvmMachinePayload {
            call_stack: EvmCallStack::new(EvmRootFrame {
                state: call.active().clone(),
            }),
            store: call.store.clone(),
            relations: call.relations.clone(),
        };
        let mut table = Table::default();
        let mut counters = Counters::default();
        let id = table
            .solver(
                &world,
                &entry,
                &config,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            )
            .procedure(root)
            .unwrap();
        assert_eq!(id, 0);
        let before = table.procedures[id]
            .entry_payload
            .clone_with_payment(&mut budget)
            .unwrap();
        let revision = table.procedures[id].entry_revision;
        let attempt = || {
            table
                .solver(
                    &world,
                    &entry,
                    &config,
                    &mut budget,
                    &mut counters,
                    StorageReadPolicy::Abstract,
                )
                .procedure(child)
        };
        let refused = if cancelled {
            let control = Control::new(Default::default());
            control.cancellation().cancel();
            control.scope(attempt)
        } else {
            attempt()
        };
        if cancelled {
            assert!(matches!(
                refused,
                Err(MachineError::Relations(QueryReason::Cancelled))
            ));
        } else {
            assert!(matches!(refused, Err(MachineError::States)));
        }
        assert_eq!(table.procedures.len(), 1);
        assert_eq!(table.procedures_by_key.len(), 1);
        assert_eq!(table.nodes.len(), 1);
        assert_eq!(table.procedures[id].entry_revision, revision);
        assert!(
            before
                .semantic_eq(
                    &table.procedures[id].entry_payload,
                    &mut PolicyContext::new(domain, &entry, &mut budget)
                )
                .unwrap()
        );
        assert!(
            table
                .runtime
                .pending_relative()
                .any(|task| task == TaskKind::Body(0))
        );
        assert_ne!(
            table.runtime.view().closure(),
            analysis_kernel::Closure::Closed
        );
    }
}
