use super::*;
use crate::Version;

struct NativeA<'a> {
    job: JobId,
    world: Scope,
    anchor: &'a SlotMeta<u8>,
    version: Version,
    allowed: u8,
}
fn a(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    shared: &mut Inputs,
    local: &mut Guarantees,
    native: NativeA<'_>,
) -> bool {
    let mut run = kernel.begin(native.job, meter).unwrap();
    assert_eq!(run.observe_slot(native.anchor).unwrap(), native.version);
    let peer = run.query(shared, Scoped::new(native.world, 1)).unwrap();
    let bound = native.allowed & peer.value().copied().unwrap_or(0b111);
    assert_eq!(
        bound & native.allowed,
        native.allowed,
        "xA=xB contains all concretely allowed values"
    );
    refine(&mut run, local, bound);
    run.publish(shared, Scoped::new(native.world, 0), native.allowed)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    peer.state == Availability::Pending
}
// B's component boundary has no A scope, selector, native input or anchor.
fn b(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    shared: &mut Inputs,
    local: &mut Guarantees,
    job: JobId,
    world: Scope,
) -> bool {
    let mut run = kernel.begin(job, meter).unwrap();
    let peer = run.query(shared, Scoped::new(world, 0)).unwrap();
    refine(&mut run, local, peer.value().copied().unwrap_or(0b111));
    if let Some(bound) = peer.value() {
        run.publish(shared, Scoped::new(world, 1), *bound).unwrap();
    }
    run.finish(LocalClosure::Stable).unwrap();
    peer.state == Availability::Pending
}
fn evaluate(s: &mut Scenario, which: usize, version: Version) -> bool {
    let f = &mut s.f;
    if which == 0 {
        a(
            &mut f.kernel,
            &mut f.meter,
            &mut s.shared,
            &mut f.local,
            NativeA {
                job: f.reader,
                world: f.source,
                anchor: &s.anchor,
                version,
                allowed: s.native,
            },
        )
    } else {
        b(
            &mut f.kernel,
            &mut f.meter,
            &mut s.shared,
            &mut f.local,
            f.downstream_reader,
            f.source,
        )
    }
}

#[test]
fn automatically_guarded_shared_feedback_closes_after_one_growth_in_both_orders() {
    for reverse in [false, true] {
        let mut f = Fixture::configured(0b010, reverse);
        let anchor = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let shared = f.kernel.channel(Ascending(Union), &mut f.meter).unwrap();
        let mut s = Scenario {
            f,
            anchor,
            native: 0b010,
            shared,
        };
        let order = if reverse { [1, 0] } else { [0, 1] };
        let mut version = Version::default();
        for which in order {
            evaluate(&mut s, which, version);
        }
        for _ in 0..8 {
            let Some(job) = s.f.kernel.next(&mut s.f.meter).unwrap() else {
                break;
            };
            let which = if job == s.f.reader {
                0
            } else {
                assert_eq!(job, s.f.downstream_reader);
                1
            };
            evaluate(&mut s, which, version);
        }
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
        // Both values remain numerically {1}; only the declared native source
        // loses validity. Provenance, rather than hidden B reads, stales the cycle.
        s.grow_to(0b110);
        let owners = [s.f.owner, s.f.downstream_owner];
        let entries = [s.f.entry.clone(), s.f.downstream_entry.clone()];
        let mut generations = [0, 0];
        let mut first_owner = None;
        let mut pending_seen = false;
        let mut steps = 0;
        while let Some(job) = s.f.kernel.next(&mut s.f.meter).unwrap() {
            steps += 1;
            assert!(
                steps < 40,
                "coverage restoration must not cause generation churn"
            );
            if let Some(which) = owners.iter().position(|owner| *owner == job) {
                if s.f
                    .kernel
                    .generation_stale(&entries[which], &mut s.f.meter)
                    .unwrap()
                {
                    first_owner.get_or_insert(which);
                    generations[which] += 1;
                    assert_eq!(generations[which], 1);
                    if which == 0 {
                        let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
                        let input = run.read_slot(&s.anchor, &s.native, 1).unwrap();
                        assert_eq!(input.value(), Some(&0b110));
                        version = input.version;
                        run.finish(LocalClosure::Stable).unwrap();
                    }
                    s.replace_with_top(which == 0, 0b111);
                    pending_seen |= evaluate(&mut s, which, version);
                } else {
                    owner_read(&mut s.f.kernel, &mut s.f.meter, job, &entries[which]);
                }
            } else {
                let which = if job == s.f.reader {
                    0
                } else {
                    assert_eq!(job, s.f.downstream_reader);
                    1
                };
                evaluate(&mut s, which, version);
            }
        }
        assert_eq!(first_owner, Some(order[0]));
        assert!(pending_seen);
        assert_eq!(generations, [1, 1]);
        for (which, owner) in owners.iter().enumerate() {
            let scope = if which == 0 {
                s.f.target
            } else {
                s.f.downstream
            };
            let mut run = s.f.kernel.begin(*owner, &mut s.f.meter).unwrap();
            assert_eq!(
                run.query(&mut s.f.local, Scoped::new(scope, 0))
                    .unwrap()
                    .value(),
                Some(&0b110)
            );
            assert_eq!(
                run.query(&mut s.shared, Scoped::new(s.f.source, which as u8))
                    .unwrap()
                    .value(),
                Some(&0b110)
            );
            run.finish(LocalClosure::Stable).unwrap();
        }
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
        for _ in 0..20 {
            for which in order {
                evaluate(&mut s, which, version);
            }
            assert_eq!(s.f.kernel.next(&mut s.f.meter).unwrap(), None);
            assert!(!s.f.stale(0) && !s.f.stale(1));
            assert_eq!(s.f.kernel.closure(), Closure::Closed);
        }
    }
}
