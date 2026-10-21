use super::*;
use crate::{Accepts, Cons, Either, Here, Nil, PublishError, Read, There, solve};

type Stores = Cons<Observed, Cons<Inputs, Cons<Inputs, Nil>>>;
type CycleError = Either<crate::KernelError<Stop>, PublishError<Infallible, Stop>>;

struct CompleteCycle {
    producer: JobId,
    consumer: JobId,
    world: Scope,
    missing: bool,
    evaluations: [usize; 2],
}
impl Analyzer<Context<'_, '_, (), Budget, Stores>> for CompleteCycle {
    type Task = ();
    type Error = CycleError;
    fn evaluate(
        &mut self,
        _: &(),
        cx: &mut Context<'_, '_, (), Budget, Stores>,
    ) -> Result<LocalClosure, CycleError> {
        if cx.run().job() == self.producer {
            self.evaluations[0] += 1;
            let acknowledgment =
                <_ as Read<Bits, There<Here>>>::query(cx, Scoped::new(self.world, 0))
                    .map_err(Either::Left)?;
            let available = if self.missing {
                <_ as Read<Bits, There<There<Here>>>>::query(cx, Scoped::new(self.world, 99))
                    .map_err(Either::Left)?
                    .value()
                    .is_some()
            } else {
                true
            };
            if acknowledgment.value() == Some(&1) && available {
                <_ as Accepts<Observations>>::accept(cx, Scoped::new(self.world, 0), COMPLETE)
                    .map_err(Either::Right)?;
            }
        } else {
            assert_eq!(cx.run().job(), self.consumer);
            self.evaluations[1] += 1;
            // Required Read adopts the already sound bound even while its
            // completion bit is false. A later completion update must not erase
            // this selected generation or its available local guarantee.
            let observed = <_ as Read<Observations>>::query(cx, Scoped::new(self.world, 0))
                .map_err(Either::Left)?;
            assert_eq!(observed.value().map(|value| value.bound), Some(1));
            <_ as Accepts<Bits, There<Here>>>::accept(cx, Scoped::new(self.world, 0), 1)
                .map_err(Either::Right)?;
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn finite_required_read_feedback_closes_but_a_missing_mandatory_input_stays_open() {
    for reverse in [false, true] {
        for missing in [false, true] {
            let mut s = Scenario::new(ObservationPolicy);
            let before = s.read_b();
            let acknowledgments =
                s.f.kernel
                    .channel(Ascending(Union), &mut s.f.meter)
                    .unwrap();
            let inputs =
                s.f.kernel
                    .channel(Ascending(Union), &mut s.f.meter)
                    .unwrap();
            let mut stores =
                Cons::new(s.values, Cons::new(acknowledgments, Cons::new(inputs, Nil)));
            let mut rule = CompleteCycle {
                producer: s.f.reader,
                consumer: s.f.downstream_reader,
                world: s.f.source,
                missing,
                evaluations: [0; 2],
            };
            let order = if reverse {
                [s.f.downstream_reader, s.f.reader]
            } else {
                [s.f.reader, s.f.downstream_reader]
            };
            for job in order {
                s.f.kernel.retry(job, &mut s.f.meter).unwrap();
            }
            let report =
                solve(&mut s.f.kernel, &mut rule, &mut stores, &mut s.f.meter, 16).unwrap();
            assert!(report.steps < 16);
            assert!(rule.evaluations.iter().all(|count| *count > 0));
            assert!(s.f.kernel.worklist().is_empty());
            assert!(!s.f.stale(0) && !s.f.stale(1));
            if missing {
                assert!(
                    matches!(report.closure, Closure::Open(open) if open.missing > 0 && open.obligations == 0)
                );
            } else {
                assert_eq!(report.closure, Closure::Closed);
                assert!(
                    rule.evaluations[1] >= 2,
                    "completion woke the required reader"
                );
            }
            s.values = stores.head;
            let after = s.read_b();
            assert_eq!(after.value(), Some(if missing { &OPEN } else { &COMPLETE }));
            assert_eq!(
                after.version.get(),
                before.version.get() + u64::from(!missing)
            );
            assert_eq!(s.guarantee().value(), Some(&1));
        }
    }
}
