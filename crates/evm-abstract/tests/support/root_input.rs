//! Carrier-generic input authority, root coverage and paid projection fixture.
use evm_abstract::{
    U256,
    analysis::{AnalysisInput, Config, ContextPolicy, EvmAnalysis, Status, control::Control},
};

/// The context policy deliberately owns a borrow and implements neither Clone
/// nor Copy. The same engine admission must work for every native carrier.
struct BorrowedContext<'a>(&'a std::cell::Cell<usize>);
impl evm_abstract::analysis::ContextPolicy for BorrowedContext<'_> {
    fn advance(&self, history: &mut Vec<usize>, source: usize, depth: usize) {
        self.0.set(self.0.get() + 1);
        evm_abstract::analysis::RecentDistinctJumps.advance(history, source, depth);
    }

    fn normalize(&self, history: &mut Vec<usize>) {
        evm_abstract::analysis::RecentDistinctJumps.normalize(history);
    }

    fn work(&self, length: usize) -> usize {
        evm_abstract::analysis::RecentDistinctJumps.work(length)
    }
}

pub fn assert_native_input_coverage<D: evm_abstract::domain::EvmNumericAlgebra>(
    values: &evm_abstract::domain::ValueAlgebra<D>,
) {
    use evm_abstract::{
        Address, Fork,
        analysis::{
            AnalysisEngine, EnginePolicies, ExecutionConfig, FirstResumeStatus, OutcomeKind,
            RootTerminalTrigger, cooperation::NoExtension,
        },
        domain::EvmValue,
        world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
    };

    let owner = Address::repeat_byte(0x41);
    let mut world = EvmWorld::<D::Element>::new(Fork::Osaka, "native carrier root coverage");
    let mut account = EvmAccount::from_hex("6003565b00", Fork::Osaka).unwrap();
    account.balance = EvmValue::constant(U256::from(10));
    world.insert(owner, account).unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::repeat_byte(0x42),
        EvmValue::constant(U256::ZERO),
        EvmByteArray::unknown(),
    );
    let scope = entry.environment.input_scope;
    let input = AnalysisInput::new(world, entry).unwrap();
    let snapshot = input.snapshot().clone();
    let revision = input.revision();
    let advances = std::cell::Cell::new(0);
    let policies = EnginePolicies::new(BorrowedContext(&advances), FirstResumeStatus);
    let mut extension = NoExtension;
    let graph = AnalysisEngine::new(values, &policies, &mut extension)
        .run(
            input,
            ExecutionConfig {
                analysis: Config {
                    max_transfers: 1,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .unwrap();

    assert_eq!(graph.status(), Status::Incomplete);
    assert_eq!(graph.transfers(), 1);
    assert!(advances.get() > 0);
    let coverage = graph
        .root_terminal_coverage()
        .expect("all carriers retain original root authority");
    assert_eq!(coverage.trigger(), &RootTerminalTrigger::Transfers);
    assert_eq!(coverage.input().snapshot(), &snapshot);
    assert_eq!(coverage.input().revision(), revision);
    assert!(coverage.input().input_scope().is_some());
    assert_eq!(coverage.input().entry().environment.input_scope, scope);
    assert_eq!(coverage.input().domain_spec(), values.spec());
    assert_eq!(
        coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
        [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure
        ]
    );
    let json = serde_json::to_value(coverage.input().call_value()).unwrap();
    assert!(json.is_object());
    assert_eq!(
        json["numeric"],
        serde_json::to_value(coverage.input().call_value().numeric()).unwrap()
    );
    assert!(json["numeric"].is_object());
    let mut unpaid_projection = evm_abstract::resource::WorkBudget::new(0);
    assert!(matches!(
        EvmAnalysis::from_execution(graph, &mut unpaid_projection),
        Err(evm_abstract::analysis::ProjectionError::Work)
    ));
    assert_eq!(unpaid_projection.used(), 0);
}
