use super::*;

#[test]
fn preserved_completion_cannot_hide_source_basis_invalidation() {
    let mut s = Scenario::new(ObservationPolicy);
    s.publish(s.f.reader, 0, COMPLETE);
    let before = s.read_b();
    s.grow_source();
    assert!(s.f.stale(0) && s.f.stale(1));
    let after = s.read_world();
    assert_eq!(after.state, Availability::Pending);
    assert_eq!(
        after.version, before.version,
        "source invalidation does not rewrite the payload"
    );
    assert_eq!(s.guarantee().state, Availability::Pending);
}

#[test]
fn preserving_completion_from_a_new_family_still_invalidates_prior_adopters() {
    let mut s = Scenario::new(ObservationPolicy);
    let c =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    assert!(ObservationPolicy.preserves_guarantees(&OPEN, &COMPLETE));
    assert_eq!(s.publish(c, 0, COMPLETE), Change::Changed);
    assert!(s.f.stale(1));
    assert!(!s.f.stale(0) && !s.f.stale(2));
    assert_eq!(s.guarantee().state, Availability::Pending);
}

#[test]
fn first_values_keep_existing_directory_and_absence_rules() {
    for absent in [false, true] {
        let mut s = Scenario::new(ObservationPolicy);
        let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
        if absent {
            run.resolve_absent(&mut s.values, Scoped::new(s.f.source, 1))
                .unwrap();
        }
        run.finish(LocalClosure::Stable).unwrap();
        // B's existing bound/completion premise is augmented with an actual
        // complete-membership observation, independently of value classification.
        let mut run =
            s.f.kernel
                .begin(s.f.downstream_reader, &mut s.f.meter)
                .unwrap();
        let keys = run.keys(&mut s.values, s.f.source).unwrap();
        assert_eq!(keys, if absent { vec![0, 1] } else { vec![0] });
        run.finish(LocalClosure::Stable).unwrap();
        s.publish(s.f.reader, 1, COMPLETE);
        // A resolved Absent is already a directory member. Its first Value
        // retains the ordinary conservative value path, without a second key.
        if absent {
            assert!(!s.f.stale(1));
        } else {
            assert!(s.f.stale(1));
        }
    }
}

#[test]
fn equivalent_republication_restores_coverage_without_reclassifying_or_staling() {
    let mut s = Scenario::new(ObservationPolicy);
    s.publish(s.f.reader, 0, COMPLETE);
    let before = s.read_b();
    s.grow_source();
    s.replace(false);
    s.replace(true);
    owner_read(&mut s.f.kernel, &mut s.f.meter, s.f.owner, &s.f.entry);
    owner_read(
        &mut s.f.kernel,
        &mut s.f.meter,
        s.f.downstream_owner,
        &s.f.downstream_entry,
    );
    assert_eq!(s.read_b().state, Availability::Pending);
    assert!(matches!(s.f.kernel.closure(), Closure::Open(open) if open.missing > 0));
    assert_eq!(s.publish(s.f.reader, 0, COMPLETE), Change::Changed);
    assert!(!s.f.stale(0) && !s.f.stale(1));
    assert!(s.f.kernel.worklist().contains(&s.f.downstream_reader));
    let after = s.read_b();
    assert_eq!(after.value(), Some(&COMPLETE));
    assert_eq!(after.version.get(), before.version.get() + 1);
    assert_eq!(s.guarantee().value(), Some(&1));
    assert_eq!(s.publish(s.f.reader, 0, COMPLETE), Change::Unchanged);
    assert_eq!(s.read_b(), after);
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}
