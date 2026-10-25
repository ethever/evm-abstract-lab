use super::{oracle, run, run_policy, support};
use evm_abstract::{
    analysis::{
        Acceleration, FrameKey, MachineError, PolicyContext, PolicySite, ProcedureOutput,
        ProcedureSummaryPolicy, RegionalSummaries, RelativeState, TransferSemantics,
    },
    domain::EvmNumericAlgebra,
};
use revm_bytecode::opcode;
use support::{A, Assembly, U, address, insert, new_world};

pub(super) struct RootRegional;

impl<D: EvmNumericAlgebra> ProcedureSummaryPolicy<D> for RootRegional {
    fn summarize_known_root(&self, frame: &FrameKey) -> bool {
        frame.code_address == address(A)
    }

    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        RegionalSummaries::new().admit_relative(current, incoming, site, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        RegionalSummaries::new().admit_output(current, incoming, acceleration, cx)
    }
}

#[test]
fn explicit_known_root_admission_uses_native_control_and_keeps_default_root_routing() {
    let mut code = Assembly::default();
    code.op(opcode::STOP)
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &code.finish(), &[]);
    let ordinary = run(world.clone(), false);
    assert!(ordinary.procedure_summaries().is_empty());
    assert_eq!(ordinary.states()[0].executed_pcs, [0]);

    let relative = run_policy(world.clone(), RootRegional, "known-root");
    assert_eq!(relative.states().len(), 1);
    assert!(relative.states()[0].executed_pcs.is_empty());
    let record = relative.procedure_summaries().last().unwrap();
    assert_eq!(record.procedures.len(), 1);
    assert_eq!(record.procedures[0].frame.code_address, address(A));
    assert!(record.procedures[0].dependencies.is_empty());
    assert!(record.regional_cfg.calls.is_empty());
    assert!(record.regional_cfg.nodes.iter().all(|node| {
        let transfer = node.transfer.as_ref().unwrap();
        transfer.current
            && transfer.complete
            && transfer.visited_block
            && transfer.semantics == TransferSemantics::Native
            && transfer.process.is_none()
    }));
    oracle::execute(&world, &support::entry(support::calldata(0))).covers(&relative);
}
