//! Actual source resolution keeps original markers distinct from executables.
use super::*;
use evm_abstract::world::Code;

#[test]
fn original_delegation_and_boundary_inputs_keep_their_declared_source_meaning() {
    let target = Address::repeat_byte(0x23);
    let precompile = Address::from_word(U256::ONE.into());
    let request = request();
    let fork = crate::analyze::input::fork(request.fork);
    let runtime = EvmAccount::<NumericValue>::from_hex("00", fork)
        .unwrap()
        .code;
    for (label, root_code, target_code) in [
        ("empty", Code::Empty, None),
        ("resolved", Code::Delegation(target), Some(runtime)),
        ("unresolved", Code::Delegation(target), None),
        (
            "delegated-empty",
            Code::Delegation(target),
            Some(Code::Empty),
        ),
        ("delegated-native", Code::Delegation(precompile), None),
        (
            "invalid-delegation",
            Code::Delegation(target),
            Some(Code::Delegation(Address::repeat_byte(0x24))),
        ),
    ] {
        let coordinate = Address::repeat_byte(0x11);
        let mut world = EvmWorld::new(fork, label);
        let mut root = EvmAccount::empty();
        root.code = root_code;
        world.insert(coordinate, root).unwrap();
        if let Some(code) = target_code {
            let mut account = EvmAccount::empty();
            account.code = code;
            world.insert(target, account).unwrap();
        }
        let mut entry = EvmEntry::new(coordinate);
        entry.environment = crate::analyze::input::environment(&request.environment).unwrap();
        let input = AnalysisInput::new(world, entry).unwrap();
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let control = Control::default();
        let source = AnalysisEngine::new(&algebra, &policies, &mut extension)
            .run_hybrid(
                input,
                crate::analyze::input::config(&request.limits).unwrap(),
                HybridConfig {
                    max_detail_work: 0,
                    ..Default::default()
                },
                &control,
            )
            .unwrap();
        let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
        let report = project(
            &source,
            &algebra,
            &control,
            &mut budget,
            ProjectionInput {
                request: &request,
                scope: api::AnalysisScope::SingleProgram,
                numeric_model: api::NumericModel::Product,
            },
        )
        .unwrap();
        report
            .validate()
            .unwrap_or_else(|error| panic!("{label}: {error:?}"));
        let model = report.minimum.as_ref().unwrap();
        let original = model.accounts.iter().find(|row| row.root_template).unwrap();
        assert!(original.program.is_none(), "{label}");
        if label == "empty" {
            assert_eq!(original.code_kind, api::CodeKind::Empty);
            assert!(matches!(
                model.declared_root_source,
                api::ProcessSource::Empty(_)
            ));
        } else {
            assert_eq!(original.code_kind, api::CodeKind::Delegation);
            match (&model.declared_root_source, label) {
                (api::ProcessSource::Runtime(source), "resolved") => {
                    assert_eq!(source.source, target.to_string());
                }
                (api::ProcessSource::InputDelegation(source), "unresolved") => {
                    assert_eq!(source.target, target.to_string());
                }
                (api::ProcessSource::InvalidDelegation(source), "invalid-delegation") => {
                    assert_eq!(source.source, target.to_string());
                }
                (api::ProcessSource::Empty(_), "delegated-empty" | "delegated-native") => {}
                (source, _) => panic!("{label}: {source:?}"),
            }
        }
        if matches!(
            model.declared_root_source,
            api::ProcessSource::InputDelegation(_)
                | api::ProcessSource::Runtime(_)
                | api::ProcessSource::InvalidDelegation(_)
        ) {
            let mut invalid = report.clone();
            Arc::make_mut(invalid.minimum.as_mut().unwrap())
                .accounts
                .iter_mut()
                .find(|row| row.root_template)
                .unwrap()
                .delegation_target = Some(Address::repeat_byte(0x77).to_string());
            assert_eq!(
                invalid.validate(),
                Err(api::ReportValidationFailure::Minimum(
                    api::MinimumValidationKind::Source
                )),
                "{label}: the original marker target remains bound even without a program reference"
            );
        }
    }
}
