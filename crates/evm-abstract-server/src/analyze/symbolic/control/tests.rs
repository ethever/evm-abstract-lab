//! Qualification, immutable seed sharing and both original cancellation owners.
use super::*;
use evm_abstract::{Fork, analysis::control::Control, bytecode::Program};

fn fixture() -> (analysis::ProcessControlRegion, Vec<Arc<api::ProgramInfo>>) {
    let program = Program::from_hex_with_fork("6003565b00", Fork::Osaka).unwrap();
    let hash = alloy_primitives::keccak256(program.bytes());
    let source = analysis::ProcessSource::InputProgram {
        hash,
        fork: Fork::Osaka,
    };
    let relation = Arc::new(analysis::ProcessControlRelation {
        epoch: B256::repeat_byte(0x11),
        source,
        blocks: vec![
            analysis::ProcessControlBlock {
                successors: [
                    Some((
                        analysis::ProcessLocation::Block(1),
                        analysis::MachineEdgeKind::Intraprocedural(analysis::EdgeKind::Jump),
                    )),
                    None,
                ],
                continuation: None,
                boundary: false,
            },
            analysis::ProcessControlBlock {
                successors: [None, None],
                continuation: None,
                boundary: true,
            },
        ],
        jump_targets: vec![1],
    });
    let programs = vec![Arc::new(api::ProgramInfo {
        id: 0,
        code_address: None,
        fork: api::Fork::Osaka,
        code_hash: hash.to_string(),
        kind: api::CodeKind::Runtime,
        bytecode: "0x6003565b00".into(),
        blocks: crate::analyze::pools::blocks(&program),
    })];
    (
        analysis::ProcessControlRegion {
            relation,
            entry: analysis::ProcessLocation::Block(0),
            through: vec![analysis::ProcessLocation::Block(0)],
            boundaries: vec![analysis::ProcessLocation::Block(1)],
        },
        programs,
    )
}

#[test]
fn relation_key_shares_equal_native_allocations_and_rejects_changed_content() {
    let (region, _) = fixture();
    let control = Control::default();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let mut pool = Pool::minimum(&control, &mut budget);
    assert_eq!(pool.control_region(&region).unwrap().relation, 0);
    let mut equal = region.clone();
    equal.relation = Arc::new((*region.relation).clone());
    assert!(!Arc::ptr_eq(&region.relation, &equal.relation));
    assert_eq!(pool.control_region(&equal).unwrap().relation, 0);
    assert_eq!(pool.control_directory.rows.len(), 1);
    let mut changed = equal.clone();
    Arc::make_mut(&mut changed.relation).blocks[0].boundary = true;
    assert_eq!(
        pool.control_region(&changed).unwrap_err().details,
        api::ErrorDetails::Report(api::ReportValidationFailure::Process(
            api::ProcessValidationKind::Source
        ))
    );
    let mut next_epoch = region.clone();
    Arc::make_mut(&mut next_epoch.relation).epoch = B256::repeat_byte(0x22);
    assert_eq!(pool.control_region(&next_epoch).unwrap().relation, 1);
    let mut other_role = region;
    let analysis::ProcessSource::InputProgram { hash, fork } = other_role.relation.source else {
        unreachable!()
    };
    Arc::make_mut(&mut other_role.relation).source = analysis::ProcessSource::Runtime {
        source: alloy_primitives::Address::repeat_byte(0x77),
        hash,
        fork,
    };
    assert_eq!(pool.control_region(&other_role).unwrap().relation, 2);
}

#[test]
fn retained_directory_seed_keeps_exact_arc_and_checks_actual_new_relation_content() {
    let (region, programs) = fixture();
    let control = Control::default();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let mut pool = Pool::minimum(&control, &mut budget);
    pool.control_region(&region).unwrap();
    let rows = pool.finish_controls(&programs).unwrap();
    assert_eq!(rows[0].program, Some(0));
    let mut next = Pool::new(&control);
    next.seed_controls(rows.iter().map(Arc::clone).collect());
    next.attach_control_budget(&mut budget);
    assert_eq!(next.control_region(&region).unwrap().relation, 0);
    let retained = next.finish_controls(&programs).unwrap();
    assert!(Arc::ptr_eq(&rows[0], &retained[0]));
    let mut invalid = region;
    Arc::make_mut(&mut invalid.relation).jump_targets.clear();
    let mut next = Pool::new(&control);
    next.seed_controls(retained);
    next.attach_control_budget(&mut budget);
    assert!(next.control_region(&invalid).is_err());
}

#[test]
fn rich_directory_refuses_before_retention_and_observes_both_cancellation_owners() {
    let (region, _) = fixture();
    let control = Control::default();
    let mut budget = WorkBudget::new(1);
    let mut pool = Pool::new(&control);
    pool.attach_control_budget(&mut budget);
    assert!(
        matches!(pool.control_region(&region).unwrap_err().details, api::ErrorDetails::Export(ref failure) if failure.kind == api::ExportFailureKind::Work)
    );
    assert!(pool.control_directory.rows.is_empty());
    for original in [false, true] {
        let passed = Control::new(Default::default());
        let payer = Control::new(Default::default());
        let mut budget = payer.scope(|| WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK));
        if original {
            payer.cancellation().cancel()
        } else {
            passed.cancellation().cancel()
        }
        let mut pool = Pool::new(&passed);
        pool.attach_control_budget(&mut budget);
        assert_eq!(
            pool.control_region(&region).unwrap_err().code,
            api::ApiErrorCode::Cancelled
        );
        assert!(pool.control_directory.rows.is_empty());
    }
}
