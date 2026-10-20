use super::*;
use crate::{Availability, Change, Closure, SlotMeta, Version};

const TOP: u8 = 0b111;
struct Rule<'a> {
    job: JobId,
    peer: &'a GenerationEntry,
    anchor: Option<(&'a SlotMeta<u8>, Version, u8)>,
    concrete: u8,
}
fn evaluate(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    local: &mut Guarantees,
    rule: Rule<'_>,
) -> bool {
    let mut run = kernel.begin(rule.job, meter).unwrap();
    let mut value = TOP;
    if let Some((anchor, snapshot_version, allowed)) = rule.anchor {
        assert_eq!(run.observe_slot(anchor).unwrap(), snapshot_version);
        value &= allowed;
    }
    let peer = run.current_generation(rule.peer).unwrap();
    let pending = peer.state == Availability::Pending;
    if let Some(peer) = peer.value()
        && let Some(guarantee) = run.query(local, Scoped::new(*peer, 0)).unwrap().value()
    {
        value &= guarantee;
    }
    // Concrete oracle: xA=xB and xA belongs to allowedA in {0,1,2}.
    // Every intermediate exported upper bound must contain every allowed value.
    assert_eq!(value & rule.concrete, rule.concrete);
    refine(&mut run, local, value);
    run.finish(LocalClosure::Stable).unwrap();
    pending
}

#[test]
fn lawful_two_way_equality_model_reseeds_once_and_converges_in_both_orders() {
    for reverse in [false, true] {
        // A0=B0={1}; allowedA grows exactly once to {1,2}. New scopes start
        // independently at Top, then A=Top & allowedA & B and B=Top & A.
        let mut f = Fixture::configured(0b010, reverse);
        let anchor = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let mut allowed = 0b010;
        let mut snapshots = [Version::default(); 2];
        let mut entries = [f.entry.clone(), f.downstream_entry.clone()];
        let owners = [f.owner, f.downstream_owner];
        let mut jobs = [f.reader, f.downstream_reader];
        let mut scopes = [f.target, f.downstream];
        let order = if reverse { [1, 0] } else { [0, 1] };
        for which in order {
            evaluate(
                &mut f.kernel,
                &mut f.meter,
                &mut f.local,
                Rule {
                    job: jobs[which],
                    peer: &entries[1 - which],
                    anchor: (which == 0).then_some((&anchor, snapshots[0], allowed)),
                    concrete: allowed,
                },
            );
        }
        assert_eq!(f.kernel.closure(), Closure::Closed);
        let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
        run.publish_prepared(&anchor, &mut allowed, 0b110, Change::Changed, 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(f.stale(0) && f.stale(1));
        let mut generations = [0, 0];
        let mut steps = 0;
        let mut saw_pending = false;
        let mut first_owner = None;
        while let Some(job) = f.kernel.next(&mut f.meter).unwrap() {
            steps += 1;
            assert!(
                steps < 32,
                "lawful finite equality must converge without identity churn"
            );
            if let Some(which) = owners.iter().position(|owner| *owner == job) {
                let mut run = f.kernel.begin(job, &mut f.meter).unwrap();
                run.current_generation(&entries[which]).unwrap();
                let stale = {
                    let (kernel, meter) = run.parts();
                    kernel.generation_stale(&entries[which], meter).unwrap()
                };
                if stale {
                    first_owner.get_or_insert(which);
                    generations[which] += 1;
                    assert_eq!(generations[which], 1);
                    if which == 0 {
                        let snapshot = run.read_slot(&anchor, &allowed, 1).unwrap();
                        assert_eq!(snapshot.value(), Some(&0b110));
                        snapshots[0] = snapshot.version;
                    }
                    let next = {
                        let (kernel, meter) = run.parts();
                        let next = scope(
                            kernel,
                            meter,
                            scopes[which].key().context,
                            generations[which],
                        );
                        let rule = kernel.register(next, (), meter).unwrap();
                        seed(kernel, meter, rule, &mut f.local, TOP);
                        kernel.seal_scope(next, meter).unwrap();
                        jobs[which] = rule;
                        next
                    };
                    run.replace_generation(&mut entries[which], next).unwrap();
                    {
                        let (kernel, meter) = run.parts();
                        kernel
                            .reclaim_channel_scope(&mut f.local, scopes[which], meter)
                            .unwrap();
                    }
                    scopes[which] = next;
                    // Run the first freshly selected component before the peer
                    // owner can reseed: Pending imports no old narrow guarantee.
                    let (kernel, meter) = run.parts();
                    saw_pending |= evaluate(
                        kernel,
                        meter,
                        &mut f.local,
                        Rule {
                            job: jobs[which],
                            peer: &entries[1 - which],
                            anchor: (which == 0).then_some((&anchor, snapshots[0], allowed)),
                            concrete: allowed,
                        },
                    );
                }
                run.finish(LocalClosure::Stable).unwrap();
            } else {
                let which = jobs
                    .iter()
                    .position(|rule| *rule == job)
                    .expect("no stale rule admitted");
                evaluate(
                    &mut f.kernel,
                    &mut f.meter,
                    &mut f.local,
                    Rule {
                        job,
                        peer: &entries[1 - which],
                        anchor: (which == 0).then_some((&anchor, snapshots[0], allowed)),
                        concrete: allowed,
                    },
                );
            }
        }
        assert_eq!(first_owner, Some(order[0]));
        assert!(saw_pending);
        assert_eq!(generations, [1, 1]);
        for which in 0..2 {
            let mut run = f.kernel.begin(owners[which], &mut f.meter).unwrap();
            assert_eq!(
                run.query(&mut f.local, Scoped::new(scopes[which], 0))
                    .unwrap()
                    .value(),
                Some(&0b110)
            );
            run.finish(LocalClosure::Stable).unwrap();
        }
        assert_eq!(f.kernel.closure(), Closure::Closed);
        for _ in 0..20 {
            let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
            run.publish_prepared(&anchor, &mut allowed, 0b110, Change::Unchanged, 1)
                .unwrap();
            run.finish(LocalClosure::Stable).unwrap();
            for which in 0..2 {
                let mut run = f.kernel.begin(owners[which], &mut f.meter).unwrap();
                assert_eq!(
                    run.replace_generation(&mut entries[which], scopes[which])
                        .unwrap(),
                    Change::Unchanged
                );
                run.finish(LocalClosure::Stable).unwrap();
                assert!(
                    !f.kernel
                        .generation_stale(&entries[which], &mut f.meter)
                        .unwrap()
                );
            }
            assert_eq!(f.kernel.next(&mut f.meter).unwrap(), None);
            assert_eq!(f.kernel.closure(), Closure::Closed);
        }
    }
}
