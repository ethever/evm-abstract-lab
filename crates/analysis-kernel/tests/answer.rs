//! Consuming follow-up queries select the complete supporting observation.

use analysis_kernel::{Answer, Ascending, Availability, Kernel, LocalClosure, Meter, Scoped};
use std::convert::Infallible;
use support::{Budget, Maximum, Numbers, Stop, scope};

#[path = "support/meter.rs"]
#[allow(dead_code)]
mod meter;
#[path = "kernel/support.rs"]
mod support;

fn observations() -> [Answer<u32>; 2] {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scopes = [
        scope(&mut kernel, &mut meter, 0),
        scope(&mut kernel, &mut meter, 1),
    ];
    let job = kernel.register(scopes[0], (), &mut meter).unwrap();
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(scopes[0], 0), 1)
        .unwrap();
    for value in [2, 3] {
        run.publish(&mut channel, Scoped::new(scopes[1], 0), value)
            .unwrap();
    }
    let answers = scopes.map(|scope| run.query(&mut channel, Scoped::new(scope, 0)).unwrap());
    run.finish(LocalClosure::Stable).unwrap();
    assert!(answers[0].version.get() > 0);
    assert_ne!(answers[0].version, answers[1].version);
    assert_ne!(answers[0].scope, answers[1].scope);
    answers
}

#[test]
fn unavailable_inputs_skip_followup_and_preserve_metadata() {
    let [source, _] = observations();
    for (state, expected) in [
        (Availability::Pending, Availability::Pending),
        (Availability::Absent, Availability::Absent),
    ] {
        let actual = Answer::<u32> {
            state,
            version: source.version,
            scope: source.scope,
        }
        .try_then(|_| -> Result<Answer<String>, Stop> {
            panic!("unavailable input invoked follow-up")
        })
        .unwrap();
        assert_eq!(
            actual,
            Answer {
                state: expected,
                version: source.version,
                scope: source.scope,
            }
        );
    }
}

#[test]
fn values_adopt_the_complete_followup_for_every_availability() {
    let [source, downstream] = observations();
    for state in [
        Availability::Pending,
        Availability::Absent,
        Availability::Value("follow-up"),
    ] {
        let expected = Answer {
            state,
            version: downstream.version,
            scope: downstream.scope,
        };
        let actual = source.clone().try_then(|value| {
            assert_eq!(value, 1);
            Ok::<_, Infallible>(expected.clone())
        });
        assert_eq!(actual, Ok(expected));
    }
}

#[test]
fn generic_callers_consume_owned_values_and_preserve_error_identity() {
    fn follow_up<V, U, E>(
        source: Answer<V>,
        f: impl FnOnce(V) -> Result<Answer<U>, E>,
    ) -> Result<Answer<U>, E> {
        source.try_then(f)
    }
    #[derive(Debug, PartialEq, Eq)]
    struct Owned<T>(T);

    let [source, downstream] = observations();
    let input = || Answer {
        state: Availability::Value(Owned(7_u32)),
        version: source.version,
        scope: source.scope,
    };
    let next = Answer {
        state: Availability::Value(Owned("follow-up")),
        version: downstream.version,
        scope: downstream.scope,
    };
    let actual = follow_up(input(), |Owned(value)| {
        assert_eq!(value, 7);
        Ok::<_, Infallible>(next)
    })
    .unwrap();
    assert_eq!(actual.state, Availability::Value(Owned("follow-up")));

    let error = Box::new(Owned("failure"));
    let identity = std::ptr::from_ref(error.as_ref());
    let actual = follow_up(input(), |_| Err::<Answer<Owned<()>>, _>(error)).unwrap_err();
    assert!(std::ptr::eq(actual.as_ref(), identity));
}
