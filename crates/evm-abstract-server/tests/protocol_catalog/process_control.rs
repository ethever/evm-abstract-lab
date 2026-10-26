//! Actual selected Process locations, shared source geometry and negative wire admission.
use super::*;
use std::sync::Arc;

fn model(code: &str) -> api::AnalysisReport {
    let mut request = bytecode(code);
    let api::AnalysisInput::Bytecode(source) = &mut request.input else {
        unreachable!()
    };
    source.address = None;
    request.limits.root_terminal_coverage = false;
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    assert!(report.minimum.as_ref().unwrap().current);
    report
}

fn receipts(
    report: &api::AnalysisReport,
) -> impl Iterator<Item = (&api::RegionalNode, &api::ProcessControlRegion)> {
    report
        .minimum
        .as_ref()
        .unwrap()
        .summaries
        .iter()
        .flat_map(|summary| &summary.relation.regional_cfg.nodes)
        .filter_map(|node| {
            node.transfer
                .as_ref()?
                .process
                .as_ref()?
                .control
                .as_ref()
                .map(|region| (node, region))
        })
}

#[test]
fn pure_interiors_and_body_locations_share_one_source_relation_without_native_visits() {
    let report = model("6003565b6007565b00");
    assert_eq!(report.process_controls.len(), 1);
    let relation = &report.process_controls[0];
    let source = report.process_program(&relation.source).unwrap();
    assert_eq!(relation.program, Some(source.id));
    assert!(source.code_address.is_none());
    assert_eq!(relation.blocks.len(), 3);
    assert_eq!(relation.jump_targets, [1, 2]);
    let (node, region) = receipts(&report).next().unwrap();
    let key = node.frame.process.as_ref().unwrap();
    assert_eq!(key.location, api::ProcessLocation::Block(0));
    let navigation = report.process_source_block(key).unwrap();
    assert_eq!(navigation.program, source.id);
    assert_eq!(report.source_block(navigation).unwrap().start_pc, 0);
    for location in [
        api::ProcessLocation::JumpTargets,
        api::ProcessLocation::End,
        api::ProcessLocation::Any,
    ] {
        let mut virtual_key = key.clone();
        virtual_key.location = location;
        assert!(report.process_source_block(&virtual_key).is_none());
    }
    assert_eq!(region.entry, key.location);
    assert_eq!(
        region.through,
        [
            api::ProcessLocation::Block(0),
            api::ProcessLocation::Block(1)
        ]
    );
    assert_eq!(region.boundaries, [api::ProcessLocation::Block(2)]);
    let summary = report
        .minimum
        .as_ref()
        .unwrap()
        .summaries
        .iter()
        .find(|summary| {
            summary.relation.regional_cfg.nodes.iter().any(|node| {
                node.frame
                    .process
                    .as_ref()
                    .is_some_and(|key| key.location == api::ProcessLocation::Block(2))
            })
        })
        .unwrap();
    let boundary = summary
        .relation
        .regional_cfg
        .nodes
        .iter()
        .find(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|key| key.location == api::ProcessLocation::Block(2))
        })
        .unwrap();
    assert!(
        summary.relation.procedures[boundary.procedure]
            .process
            .as_ref()
            .unwrap()
            .same_context(boundary.frame.process.as_ref().unwrap())
    );
    assert_ne!(
        summary.relation.procedures[boundary.procedure]
            .process
            .as_ref()
            .unwrap()
            .location,
        boundary.frame.process.as_ref().unwrap().location
    );
    assert!(
        node.transfer
            .as_ref()
            .unwrap()
            .successors
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::ProcessControl)
    );
    for summary in &report.minimum.as_ref().unwrap().summaries {
        for node in &summary.relation.regional_cfg.nodes {
            assert!(node.frame.basic_block.is_none());
            assert!(
                node.transfer
                    .as_ref()
                    .is_none_or(|transfer| !transfer.visited_block)
            );
        }
    }
    // The raw relation is a directory row, never repeated inside receipts.
    let wire = serde_json::to_string(&report).unwrap();
    assert_eq!(wire.matches("\"jump_targets\"").count(), 1);
    let decoded: api::AnalysisReport = serde_json::from_str(&wire).unwrap();
    decoded.validate().unwrap();
}

#[test]
fn unknown_jump_origins_keep_one_set_valued_hub_and_real_loop_or_boundary_receipts() {
    let report = model("36565b36565b00");
    let relation = &report.process_controls[0];
    assert_eq!(relation.jump_targets, [1, 2]);
    let regions: Vec<_> = receipts(&report).map(|(_, region)| region).collect();
    let origin = regions
        .iter()
        .find(|region| region.entry == api::ProcessLocation::Block(0))
        .unwrap();
    assert_eq!(origin.through, [api::ProcessLocation::Block(0)]);
    assert_eq!(origin.boundaries, [api::ProcessLocation::JumpTargets]);
    let hub = regions
        .iter()
        .find(|region| region.entry == api::ProcessLocation::JumpTargets)
        .unwrap();
    assert!(hub.through.contains(&api::ProcessLocation::JumpTargets));
    assert!(hub.through.contains(&api::ProcessLocation::Block(1)));
    assert_eq!(hub.boundaries, [api::ProcessLocation::Block(2)]);
    assert!(regions.iter().all(|region| region.relation == relation.id));
    let loop_report = model("5b600056");
    let (_, loop_region) = receipts(&loop_report).next().unwrap();
    assert_eq!(loop_region.through, [api::ProcessLocation::Block(0)]);
    assert!(loop_region.boundaries.is_empty());
    assert!(
        loop_report
            .minimum
            .as_ref()
            .unwrap()
            .summaries
            .iter()
            .any(|summary| summary.bounds.iter().any(|bounds| bounds
                .outputs
                .iter()
                .any(|output| output.kind == api::OutcomeKind::Failure)))
    );
}

#[test]
fn process_control_admission_rejects_directory_receipt_source_epoch_and_location_corruption() {
    let report = model("6003565b6007565b00");
    for case in 0..10 {
        let mut invalid = report.clone();
        match case {
            0 => {
                Arc::make_mut(&mut invalid.process_controls[0]).epoch = api::ProcessDigest([7; 32])
            }
            1 => Arc::make_mut(&mut invalid.process_controls[0])
                .jump_targets
                .clear(),
            2 => Arc::make_mut(&mut invalid.process_controls[0]).program = None,
            3 => {
                Arc::make_mut(&mut invalid.process_controls[0]).blocks[0].successors[0]
                    .as_mut()
                    .unwrap()
                    .to = api::ProcessLocation::Block(99)
            }
            4 => Arc::make_mut(&mut invalid.process_controls[0]).blocks[1].boundary = true,
            _ => {
                let model = Arc::make_mut(invalid.minimum.as_mut().unwrap());
                let node = model
                    .summaries
                    .iter_mut()
                    .flat_map(|summary| &mut summary.relation.regional_cfg.nodes)
                    .find(|node| {
                        node.transfer
                            .as_ref()
                            .and_then(|transfer| transfer.process.as_ref())
                            .is_some_and(|process| process.control.is_some())
                    })
                    .unwrap();
                let region = node
                    .transfer
                    .as_mut()
                    .unwrap()
                    .process
                    .as_mut()
                    .unwrap()
                    .control
                    .as_mut()
                    .unwrap();
                match case {
                    5 => region.relation = 99,
                    6 => region.entry = api::ProcessLocation::JumpTargets,
                    7 => region.through.pop().map(|_| ()).unwrap(),
                    8 => region.boundaries.clear(),
                    9 => node.frame.process.as_mut().unwrap().owner = api::ProcessOwner::Residual,
                    _ => unreachable!(),
                }
            }
        }
        assert!(
            invalid.validate().is_err(),
            "corruption {case} was accepted"
        );
    }
    let mut wrong_source = report.clone();
    let mut source = (*wrong_source.programs[0]).clone();
    source.id = wrong_source.programs.len();
    source.code_address = Some(Address::repeat_byte(0x77).to_string());
    let id = source.id;
    wrong_source.programs.push(Arc::new(source));
    Arc::make_mut(&mut wrong_source.process_controls[0]).program = Some(id);
    assert!(
        wrong_source.validate().is_err(),
        "equal bytes at another source are not the directory source"
    );
}

#[test]
fn actual_owner_and_static_contexts_share_geometry_without_merging_body_receipts() {
    for namespace in [false, true] {
        let root = Address::repeat_byte(0x11);
        let callee = Address::from_word(evm_abstract::U256::from(0x201).into());
        let code = "6000600060006000600061020161fffff1600060006000600061020161fffffa600060006000600061020161fffff400";
        let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "shared MAY context geometry");
        world
            .insert(root, EvmAccount::from_hex(code, Fork::Osaka).unwrap())
            .unwrap();
        world
            .insert(
                callee,
                EvmAccount::from_hex("6003565b6007565b00", Fork::Osaka).unwrap(),
            )
            .unwrap();
        let mut request = bytecode(code);
        request.limits.root_terminal_coverage = false;
        let mut entry = EvmEntry::new(root);
        if namespace {
            entry.environment.to = evm_abstract::world::AddressInput::unknown_to();
        }
        let report = evm_abstract_server::analyze::captured::analyze(
            AnalysisInput::new(world, entry).unwrap(),
            &request,
            &Control::default(),
        )
        .unwrap();
        report.validate().unwrap();
        assert!(report.hybrid.as_ref().unwrap().coverage.baseline_current);
        let directories: Vec<_> = report.process_controls.iter().filter(|relation| matches!(&relation.source, api::ProcessSource::Runtime(source) if source.source == callee.to_string())).collect();
        assert_eq!(
            directories.len(),
            1,
            "owner/static contexts do not duplicate raw source geometry"
        );
        let directory = directories[0];
        let mut contexts = std::collections::BTreeSet::new();
        for summary in &report.global.as_ref().unwrap().abstract_summaries {
            for node in &summary.regional_cfg.nodes {
                let Some(region) = node
                    .transfer
                    .as_ref()
                    .and_then(|transfer| transfer.process.as_ref())
                    .and_then(|process| process.control.as_ref())
                else {
                    continue;
                };
                if region.relation != directory.id {
                    continue;
                }
                let key = node.frame.process.as_ref().unwrap();
                assert_eq!(key.epoch, directory.epoch);
                assert_eq!(key.source, directory.source);
                assert!(node.frame.basic_block.is_none());
                assert!(node.frame.storage_address.is_none());
                assert_eq!(node.frame.is_static, key.is_static);
                let owner = match &key.owner {
                    api::ProcessOwner::RootNamespace => (0, String::new()),
                    api::ProcessOwner::Named(owner) => (1, owner.clone()),
                    api::ProcessOwner::Residual => (2, String::new()),
                };
                contexts.insert((owner, key.is_static));
            }
        }
        let parent = if namespace {
            (0, String::new())
        } else {
            (1, root.to_string())
        };
        assert!(
            contexts.contains(&(parent, false)),
            "DELEGATECALL keeps the parent owner"
        );
        assert!(
            contexts.contains(&((1, callee.to_string()), false)),
            "CALL has its own physical owner"
        );
        assert!(
            contexts.contains(&((1, callee.to_string()), true)),
            "STATICCALL keeps read-only mode"
        );
        assert!(
            contexts.contains(&((2, String::new()), false)),
            "a residual delegated owner retains the same captured executable source"
        );
    }
}
