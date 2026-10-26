//! Application policy changes precision on the same captured nested World.
use alloy_primitives::{Address, U256};
use evm_abstract::{
    Fork,
    analysis::{AnalysisInput, control::Control},
    domain::{EvmValue, NumericValue},
    world::{EvmAccount, EvmEntry, EvmWorld, Existence},
};
use evm_abstract_protocol as api;
use evm_abstract_server::analyze::captured;

const WORLD: &str = include_str!("../../../examples/worlds/nested-detail-regions.json");

fn original() -> AnalysisInput<NumericValue> {
    let fixture: serde_json::Value = serde_json::from_str(WORLD).unwrap();
    let mut world = EvmWorld::new(Fork::Osaka, fixture["provenance"].as_str().unwrap());
    for row in fixture["accounts"].as_array().unwrap() {
        let mut account = EvmAccount::from_hex(row["code"].as_str().unwrap(), Fork::Osaka).unwrap();
        account.storage_unknown = row["storage_unknown"].as_bool().unwrap();
        assert!(row["storage"].as_object().unwrap().is_empty());
        account.balance =
            EvmValue::constant(row["balance"].as_str().unwrap().parse::<U256>().unwrap());
        account.nonce = EvmValue::constant(row["nonce"].as_str().unwrap().parse::<U256>().unwrap());
        account.existence = Existence::Unknown;
        world
            .insert(row["address"].as_str().unwrap().parse().unwrap(), account)
            .unwrap();
    }
    AnalysisInput::new(world, EvmEntry::new(Address::repeat_byte(0x11))).unwrap()
}

fn run(input: AnalysisInput<NumericValue>, request: &api::AnalyzeRequest) -> api::AnalysisReport {
    let report = captured::analyze(input, request, &Control::default()).unwrap();
    report.validate().unwrap();
    report
}

#[test]
fn same_original_nested_world_uses_zero_one_two_three_regions_without_new_budgets() {
    let source = original();
    let original = api::AnalyzeRequest::default();
    let mut baseline_work = None;
    for bound in [0, 1, 2, 3] {
        let mut request = original.clone();
        request.limits.max_detail_regions = bound;
        assert_eq!(request.limits.max_work, 2_000_000_000);
        assert_eq!(request.limits.max_transfers, 10_000_000);
        assert_eq!(request.environment, original.environment);
        let report = run(source.clone(), &request);
        let coverage = &report.hybrid.as_ref().unwrap().coverage;
        assert!(coverage.baseline_current);
        assert_eq!(coverage.requested_detail_regions, bound);
        assert_eq!(coverage.work.total, report.work);
        assert!(coverage.work.total <= request.limits.max_work);
        assert_eq!(
            *baseline_work.get_or_insert(coverage.work.baseline),
            coverage.work.baseline
        );
        assert_eq!(coverage.detail_regions.len(), bound as usize);
        println!(
            "bound={bound} baseline={} preparation={} native={} total={}",
            coverage.work.baseline,
            coverage.work.preparation,
            coverage.work.detail,
            coverage.work.total
        );
        if bound == 0 {
            assert_eq!(coverage.detail, api::HybridDetailState::Disabled);
            assert_eq!(coverage.chosen, api::HybridInterpretation::Process);
            assert!(report.native.is_none());
            assert_eq!(coverage.work.preparation + coverage.work.detail, 0);
            continue;
        }
        let native = report.native.as_ref().unwrap();

        for (index, region) in coverage.detail_regions.iter().enumerate() {
            let address = Address::repeat_byte(0x11 * (index as u8 + 1)).to_string();
            assert_eq!(region.code_address.as_deref(), Some(address.as_str()));
        }
        for state in &native.cfg {
            assert_eq!(state.code_address, state.storage_address);
            let program = report
                .programs
                .iter()
                .find(|program| Some(program.id) == state.program)
                .unwrap();
            assert_eq!(state.code_address, program.code_address);
        }
        assert!(
            native
                .edges
                .iter()
                .any(|edge| edge.kind == api::EdgeKind::Call)
        );
        for index in 1_u8..=3 {
            let address = Address::repeat_byte(0x11 * index).to_string();
            let frames: Vec<_> = native
                .cfg
                .iter()
                .filter(|state| state.code_address.as_deref() == Some(address.as_str()))
                .collect();
            if u64::from(index) <= bound {
                let first = frames
                    .iter()
                    .find(|state| state.basic_block == Some(0))
                    .unwrap();
                assert_eq!(first.executed_pcs, [0, 1, 3, 5, 6]);
                assert_eq!(first.frame_depth, usize::from(index));
                let private = native
                    .states
                    .iter()
                    .find(|state| state.state == first.id)
                    .unwrap();
                assert_eq!(private.entry.frames.len(), usize::from(index));
                assert!(private.entry.frames.last().unwrap().memory.is_some());
                assert!(private.exit.is_some());
                assert!(
                    report
                        .hybrid
                        .as_ref()
                        .unwrap()
                        .local_prefixes
                        .iter()
                        .any(|prefix| prefix.state == first.id
                            && prefix.context_selected
                            && !prefix.instructions.is_empty())
                );
            } else {
                assert!(frames.iter().all(|state| state.executed_pcs.is_empty()));
            }
        }
        let calls: Vec<_> = native
            .edges
            .iter()
            .filter(|edge| edge.kind == api::EdgeKind::Call)
            .collect();
        assert_eq!(calls.len(), bound.min(2) as usize);
        for edge in calls {
            let from = &native.cfg[edge.from];
            let to = &native.cfg[edge.to];
            assert_eq!(from.frame_depth + 1, to.frame_depth);
            assert_ne!(from.program, to.program);
            let returned = native
                .edges
                .iter()
                .find(|returned| {
                    returned.from == to.id && returned.kind == api::EdgeKind::SummaryReturn
                })
                .or_else(|| {
                    native.edges.iter().find(|returned| {
                        returned.kind == api::EdgeKind::Return
                            && native.cfg[returned.from].program == to.program
                            && native.cfg[returned.to].program == from.program
                    })
                })
                .unwrap();
            assert_eq!(native.cfg[returned.to].frame_depth, from.frame_depth);
        }
        let mut corrupt = report.clone();
        corrupt
            .hybrid
            .as_mut()
            .unwrap()
            .coverage
            .requested_detail_regions = 0;
        assert!(matches!(
            corrupt.validate(),
            Err(api::ReportValidationFailure::Hybrid(
                api::HybridValidationKind::Coverage
            ))
        ));
        let mut echoed = report.clone();
        echoed
            .hybrid
            .as_mut()
            .unwrap()
            .coverage
            .requested_detail_regions = bound + 1;
        assert!(
            matches!(
                echoed.validate(),
                Err(api::ReportValidationFailure::Hybrid(
                    api::HybridValidationKind::Coverage
                ))
            ),
            "request metadata and actual applied policy must agree"
        );
        let mut missing = report.clone();
        missing.programs.pop();
        assert!(
            missing.validate().is_err(),
            "selected source needs its actual catalog entry"
        );
    }
}

#[test]
fn omitted_and_explicit_one_keep_the_original_effective_policy_and_execution() {
    let explicit = api::AnalyzeRequest::default();
    let mut historical = serde_json::to_value(&explicit).unwrap();
    historical["limits"]
        .as_object_mut()
        .unwrap()
        .remove("max_detail_regions");
    let omitted: api::AnalyzeRequest = serde_json::from_value(historical).unwrap();
    assert_eq!(omitted, explicit);
    let source = original();
    let a = run(source.clone(), &omitted);
    let b = run(source, &explicit);
    assert_eq!(a.hybrid, b.hybrid);
    assert_eq!(a.work, b.work);
    assert_eq!(a.transfers, b.transfers);
    assert_eq!(
        a.native.as_ref().unwrap().cfg,
        b.native.as_ref().unwrap().cfg
    );
}
