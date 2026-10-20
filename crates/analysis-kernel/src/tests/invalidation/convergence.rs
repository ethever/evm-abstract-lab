use super::*;
use crate::{Availability, Change, Closure, KernelError};

struct Rule<'a> {
    job: JobId,
    peer: &'a GenerationEntry,
    limit: u8,
}
fn evaluate(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    inputs: &mut Inputs,
    local: &mut Guarantees,
    source: Scope,
    rule: Rule<'_>,
) -> bool {
    let mut run = kernel.begin(rule.job, meter).unwrap();
    let input = *run
        .query(inputs, Scoped::new(source, 0))
        .unwrap()
        .value()
        .unwrap();
    let peer = run.current_generation(rule.peer).unwrap();
    let pending = peer.state == Availability::Pending;
    let mut value = input & rule.limit;
    if let Some(peer) = peer.value()
        && let Some(guarantee) = run.query(local, Scoped::new(*peer, 0)).unwrap().value()
    {
        value &= guarantee;
    }
    refine(&mut run, local, value);
    run.finish(LocalClosure::Stable).unwrap();
    pending
}

#[test]
fn two_way_generations_reseed_once_after_growth_then_descend_to_finite_closure() {
    let mut f = Fixture::new();
    f.adopt();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.current_generation(&f.downstream_entry).unwrap();
    run.query(&mut f.local, Scoped::new(f.downstream, 0))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap(); // A reads B and B already reads A.
    let mut entries = [f.entry.clone(), f.downstream_entry.clone()];
    let owners = [f.owner, f.downstream_owner];
    let mut jobs = [f.reader, f.downstream_reader];
    let mut scopes = [f.target, f.downstream];
    let mut generations = [0, 0];
    assert_eq!(f.kernel.closure(), Closure::Closed);
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        0,
        7,
    );
    assert!(f.stale(0) && f.stale(1));
    let mut saw_stale_peer = false;
    let mut steps = 0;
    while let Some(job) = f.kernel.next(&mut f.meter).unwrap() {
        steps += 1;
        assert!(
            steps < 32,
            "A <-> B must converge without generation ping-pong"
        );
        if let Some(which) = owners.iter().position(|owner| *owner == job) {
            let mut run = f.kernel.begin(job, &mut f.meter).unwrap();
            let selected = run.current_generation(&entries[which]).unwrap();
            let stale = {
                let (kernel, meter) = run.parts();
                kernel.generation_stale(&entries[which], meter).unwrap()
            };
            if stale {
                assert_eq!(selected.state, Availability::Pending);
                generations[which] += 1;
                assert_eq!(
                    generations[which], 1,
                    "one upstream growth requires one reseed per owner"
                );
                let universe = *run
                    .query(&mut f.inputs, Scoped::new(f.source, 0))
                    .unwrap()
                    .value()
                    .unwrap();
                let next = {
                    let (kernel, meter) = run.parts();
                    let next = scope(
                        kernel,
                        meter,
                        scopes[which].key().context,
                        generations[which],
                    );
                    let rule = kernel.register(next, (), meter).unwrap();
                    seed(kernel, meter, rule, &mut f.local, universe);
                    kernel.seal_scope(next, meter).unwrap();
                    jobs[which] = rule;
                    next
                };
                run.replace_generation(&mut entries[which], next).unwrap();
                {
                    let (kernel, meter) = run.parts();
                    assert_eq!(
                        kernel
                            .reclaim_channel_scope(&mut f.local, scopes[which], meter)
                            .unwrap(),
                        1
                    );
                    assert!(matches!(
                        kernel.begin(
                            if which == 0 {
                                f.reader
                            } else {
                                f.downstream_reader
                            },
                            meter
                        ),
                        Err(KernelError::InvalidJob)
                    ));
                }
                scopes[which] = next;
                if which == 0 {
                    // Force the real interleaving A1 -> stale B0 before B's owner
                    // can replace B0. A1 must see Pending and acquire no B0 value.
                    let (kernel, meter) = run.parts();
                    saw_stale_peer |= evaluate(
                        kernel,
                        meter,
                        &mut f.inputs,
                        &mut f.local,
                        f.source,
                        Rule {
                            job: jobs[0],
                            peer: &entries[1],
                            limit: 3,
                        },
                    );
                } else {
                    let (kernel, meter) = run.parts();
                    kernel.retry(jobs[which], meter).unwrap();
                }
            }
            run.finish(LocalClosure::Stable).unwrap();
        } else {
            let which = jobs
                .iter()
                .position(|rule| *rule == job)
                .expect("no obsolete rule admitted");
            evaluate(
                &mut f.kernel,
                &mut f.meter,
                &mut f.inputs,
                &mut f.local,
                f.source,
                Rule {
                    job,
                    peer: &entries[1 - which],
                    limit: if which == 0 { 3 } else { 7 },
                },
            );
        }
    }
    assert!(saw_stale_peer);
    assert_eq!(generations, [1, 1]);
    assert_eq!(f.kernel.closure(), Closure::Closed);
    for which in 0..2 {
        assert!(
            !f.kernel
                .generation_stale(&entries[which], &mut f.meter)
                .unwrap()
        );
        let mut run = f.kernel.begin(owners[which], &mut f.meter).unwrap();
        assert_eq!(
            run.current_generation(&entries[which]).unwrap().value(),
            Some(&scopes[which])
        );
        assert_eq!(
            run.query(&mut f.local, Scoped::new(scopes[which], 0))
                .unwrap()
                .value(),
            Some(&3),
            "the enlarged universe retains bit 1 rather than the old narrowed singleton"
        );
        run.finish(LocalClosure::Stable).unwrap();
    }
    // Equivalent upstream publications and selector refreshes cannot restart
    // either component, even after they adopted each other's descending facts.
    for _ in 0..20 {
        publish(
            &mut f.kernel,
            &mut f.meter,
            f.writer,
            &mut f.inputs,
            f.source,
            0,
            7,
        );
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
