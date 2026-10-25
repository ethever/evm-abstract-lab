use super::{admission::RootRegional, native, oracle, run_policy, support};
use alloy_primitives::U256;
use evm_abstract::analysis::OutcomeKind;
use revm_bytecode::opcode;
use support::{A, Assembly, address, insert, new_world, word};

#[test]
fn known_root_and_same_context_self_delegate_keep_calldata_origin_and_outer_rollback() {
    for rollback in [false, true] {
        let mut code = Assembly::default();
        // Storage distinguishes the actual child execution without changing its
        // entry FrameKey or concretizing the original Unknown calldata.
        code.push(0).op(opcode::SLOAD).jump("child", true);
        code.store(0, 1)
            .push(77)
            .mstore(0)
            .call(opcode::DELEGATECALL, A as u16, 0, (0, 32), (32, 32))
            .mstore(64)
            .input(0)
            .mstore(96);
        if rollback {
            code.push(96).push(32).op(opcode::REVERT);
        } else {
            code.return_range(32, 96);
        }
        code.label("child").store(0, 9).input(0).return_top();
        let code = code.finish();
        let mut world = new_world();
        insert(&mut world, A, &code, &[(0, 0)]);
        let report = run_policy(world.clone(), RootRegional, "self-delegate-origin");
        let record = report.procedure_summaries().last().unwrap();
        assert_eq!(record.procedures.len(), 2);
        let root = record.root_procedure.unwrap();
        let child = *record.procedures[root].dependencies.first().unwrap();
        assert_ne!(root, child);
        assert_eq!(
            record.procedures[root].frame,
            record.procedures[child].frame
        );
        assert_eq!(
            record.procedures[root].version.namespace,
            record.procedures[child].version.namespace
        );
        assert_eq!(
            record.procedures[child].frame.caller,
            evm_abstract::world::AddressInput::unknown_caller()
        );
        assert!(record.procedures[child].dependencies.is_empty());
        assert!(record.regional_cfg.calls.iter().any(|call| {
            call.callee == child
                && record.regional_cfg.nodes[call.from].procedure == root
                && call
                    .deliveries
                    .iter()
                    .any(|delivery| delivery.kind == OutcomeKind::Return && delivery.current)
        }));

        let expanded =
            record.materialize(native::product().borrowed(), &mut native::postprocessing());
        assert_eq!(
            expanded.outcome,
            evm_abstract::analysis::ProcedureReportOutcome::Complete
        );
        let mut child_returns = 0;
        for output in expanded.procedures[child]
            .outputs
            .iter()
            .filter(|output| output.kind == OutcomeKind::Return)
        {
            child_returns += 1;
            // This bound comes from the child's actual private calldata, not
            // from the original root's Unknown environment input.
            assert_eq!(
                output
                    .data
                    .read_word(&word(0), native::product().borrowed())
                    .singleton(),
                Some(U256::from(77))
            );
        }
        assert!(child_returns > 0);

        let kind = if rollback {
            OutcomeKind::Revert
        } else {
            OutcomeKind::Return
        };
        let mut outputs = 0;
        for output in report
            .outcomes()
            .iter()
            .filter(|output| output.kind == kind)
        {
            outputs += 1;
            assert_eq!(output.data.len().singleton(), Some(U256::from(96)));
            if rollback {
                assert_eq!(
                    output
                        .store
                        .read(address(A), &word(0), native::product().borrowed())
                        .singleton(),
                    Some(U256::ZERO)
                );
            }
        }
        assert!(outputs > 0);
        for selector in [0, 1, 91] {
            let observed = oracle::execute(&world, &support::entry(support::calldata(selector)));
            assert_eq!(observed.kind, kind);
            assert_eq!(observed.output_word(0), U256::from(77));
            assert_eq!(observed.output_word(32), U256::ONE);
            assert_eq!(observed.output_word(64), U256::from(selector));
            assert_eq!(
                observed.slot(A, 0),
                U256::from(if rollback { 0 } else { 9 })
            );
            assert!(
                observed
                    .steps
                    .iter()
                    .any(|step| step.code == address(A) && step.pc == code.labels["child"])
            );
            observed.covers(&report);
        }
    }
}
