use super::*;
use crate::domain::ProductAlgebra;
use crate::{
    Fork,
    analysis::{EnginePolicies, Status, engine::policy::RecentDistinctJumps},
    domain::{NumericValue, ValueAlgebra},
    world::EvmAccount,
};
use revm_bytecode::opcode;

pub(in crate::analysis::transfer) mod words;
use words::{COPIES, Heavy, Policy, copies, reset};

const OWNED_BYTES: usize = 16_384;

fn fixture(
    code: &str,
) -> (
    EvmWorld<Heavy>,
    EvmEntry<Heavy>,
    ExecutionConfig,
    ValueAlgebra<Policy>,
) {
    let owner = Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "transfer clone admission");
    let mut account = EvmAccount::from_hex(code, Fork::Osaka).unwrap();
    account.storage_unknown = false;
    world.insert(owner, account).unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::repeat_byte(0x22),
        EvmValue::from_numeric(Heavy::payload(
            NumericValue::constant(U256::ONE),
            OWNED_BYTES,
        )),
        EvmByteArray::empty(),
    );
    let config = ExecutionConfig {
        max_work: 10_000_000,
        root_terminal_coverage: false,
        use_summaries: false,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let domain = ProductAlgebra::from_spec(config_numeric.spec());
    (world, entry, config, ValueAlgebra::new(Policy(domain)))
}

#[test]
fn public_dup_loop_cannot_converge_below_its_copy_bill() {
    let _copies = COPIES.scope();
    let run = |pairs: usize, max_work| {
        let code = format!("34{}00", "8050".repeat(pairs));
        let (world, entry, mut config, domain) = fixture(&code);
        config.max_work = max_work;
        reset();
        let report = crate::analysis::AnalysisEngine::new(
            &domain,
            &EnginePolicies::default(),
            &mut crate::analysis::cooperation::NoExtension,
        )
        .run(
            crate::analysis::AnalysisInput::new(world, entry).unwrap(),
            config,
            &crate::analysis::control::Control::default(),
        )
        .unwrap();
        let observed = copies();
        (report, observed)
    };
    let (base, base_copies) = run(0, 10_000_000);
    assert_eq!(base.status(), Status::Converged);
    let (limited, _) = run(1_000, 10_000_000);
    assert_eq!(limited.status(), Status::Incomplete);
    assert!(
        limited
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(limited.work() <= 10_000_000);
    let (complete, complete_copies) = run(1_000, 100_000_000);
    assert_eq!(complete.status(), Status::Converged);
    assert_eq!(
        complete_copies.bytes - base_copies.bytes,
        1_000 * OWNED_BYTES
    );
    assert!(complete_copies.quotes - base_copies.quotes >= 1_000);
    assert!(complete.work() - base.work() >= 1_000 * OWNED_BYTES);
    eprintln!(
        "DUP resource witness: base_work={} base_copied_bytes={} base_quotes={} limited_work={} limited_status={:?} complete_work={} complete_copied_bytes={} complete_quotes={}",
        base.work(),
        base_copies.bytes,
        base_copies.quotes,
        limited.work(),
        limited.status(),
        complete.work(),
        complete_copies.bytes,
        complete_copies.quotes
    );
}

#[test]
fn rejected_dup_keeps_the_selected_stack_value_without_copying_it() {
    let _copies = COPIES.scope();
    let (world, entry, config, domain) = fixture("8000");
    let mut input = initial(&world, &entry, domain.borrowed()).unwrap();
    input
        .active_mut()
        .stack
        .push(EvmValue::from_numeric(Heavy::payload(
            NumericValue::constant(U256::ONE),
            OWNED_BYTES,
        )));
    input.normalize();
    let touch = (usize::BITS as usize + 1) * 16;
    let mut budget = WorkBudget::new(touch + 1);
    reset();
    let result = execute(
        &world,
        &entry,
        input,
        &config,
        domain.borrowed(),
        &RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert!(budget.exhausted());
    assert_eq!(result.executed_pcs, vec![0]);
    assert_eq!(
        result.instruction_progress,
        vec![InstructionProgress::Started]
    );
    assert_eq!(result.payload.active().stack.len(), 1);
    assert_eq!(
        result.payload.active().stack[0].singleton(),
        Some(U256::ONE)
    );
    assert!(result.instruction_values.is_empty());
    assert!(result.successors.is_empty() && result.outcomes.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(copies().quotes, 1);
    assert_eq!(copies().bytes, 0);
    assert_eq!(copies().bytes, copies().bytes_at_last_quote);
}

#[test]
fn environment_and_receipt_copies_are_separately_admitted_before_output() {
    let _copies = COPIES.scope();
    let (world, entry, config, domain) = fixture("3400");
    let make_input = || initial(&world, &entry, domain.borrowed()).unwrap();
    let input = make_input();
    let env_copy = input.active().call_value.clone_work();
    let program = input.active().program.as_ref().unwrap();
    let operation = budget::operation_work(
        &empty_execution(make_input()),
        opcode::CALLVALUE,
        &[],
        program,
        domain.borrowed(),
        &config,
        &entry,
    );
    let touch = (usize::BITS as usize + 1) * 16;
    for (remaining_copy_work, quotes, minimum_copies) in [(0, 1, 0), (env_copy, 2, OWNED_BYTES)] {
        let mut budget = WorkBudget::new(touch + 1 + operation + remaining_copy_work);
        let input = make_input();
        reset();
        let result = execute(
            &world,
            &entry,
            input,
            &config,
            domain.borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(budget.exhausted());
        assert_eq!(result.executed_pcs, vec![0]);
        assert_eq!(
            result.instruction_progress,
            vec![InstructionProgress::OperandsConsumed]
        );
        assert!(result.payload.active().stack.is_empty());
        assert!(result.instruction_values.is_empty());
        assert!(result.successors.is_empty() && result.outcomes.is_empty());
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert_eq!(copies().quotes, quotes);
        assert!(copies().bytes >= minimum_copies);
        assert_eq!(copies().bytes, copies().bytes_at_last_quote);
    }
}
