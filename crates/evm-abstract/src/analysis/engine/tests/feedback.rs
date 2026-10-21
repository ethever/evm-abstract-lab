use crate::domain::ProductAlgebra;
//! A covered arrival may still establish a new acceleration obligation.

use super::{Counters, engine, fixtures};
use crate::domain::EvmNumericAlgebra as _;
use crate::{
    analysis::{FrontierReason, MachineEdgeKind, engine::Successor, worklist::kernel_failure},
    domain::ValueAlgebra,
    resource::WorkBudget,
};

#[test]
fn first_feedback_wakes_a_covered_input_and_invalidates_its_receipt_once() {
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    let result = fixtures::run(
        "00",
        config,
        crate::analysis::testing::NumericLimits::machine().spec(),
    );
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = Default::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(result, 0, &mut budget, &mut counters, &algebra, &policies);
    engine.reset_pending(&[]);
    let input = engine.result.states[0].entry.clone();
    let history = engine.alternatives[0].work_size();
    assert!(
        engine.result.states[0]
            .execution_evidence()
            .unwrap()
            .is_current()
    );

    for expected_version in [1, 1] {
        assert_eq!(
            engine.successor(
                0,
                Successor {
                    payload: input.clone(),
                    kind: MachineEdgeKind::Intraprocedural(crate::analysis::EdgeKind::Jump)
                },
                FrontierReason::Work,
            ),
            Some(0),
        );
        let version = engine
            .runtime
            .with_run(engine.active.as_mut().unwrap(), engine.budget, |run| {
                run.slot_version(&engine.inputs[0]).map_err(kernel_failure)
            })
            .unwrap();
        assert_eq!(version.get(), expected_version);
        assert!(engine.feedback[0]);
        assert_eq!(engine.updates[0], 0);
        assert_eq!(engine.result.states[0].entry, input);
        assert_eq!(engine.alternatives[0].work_size(), history);
        assert!(
            !engine.result.states[0]
                .execution_evidence()
                .unwrap()
                .is_current()
        );
        assert!(engine.runtime.contains_world(&0));
    }
}
