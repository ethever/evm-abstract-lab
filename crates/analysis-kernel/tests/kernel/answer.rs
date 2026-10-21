use super::support::{Budget, Maximum, Numbers, scope};
use analysis_kernel::{Answer, Ascending, Availability, Kernel, LocalClosure, Scoped};
use std::convert::Infallible;

#[test]
fn try_then_preserves_unavailable_inputs_and_adopts_complete_followups() {
    #[derive(Debug, PartialEq, Eq)]
    struct Owned<T>(T);

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
    let [source, downstream] =
        scopes.map(|scope| run.query(&mut channel, Scoped::new(scope, 0)).unwrap());
    run.finish(LocalClosure::Stable).unwrap();
    assert!(source.version.get() > 0);
    assert_ne!(source.version, downstream.version);
    assert_ne!(source.scope, downstream.scope);
    let input = |state| Answer {
        state,
        version: source.version,
        scope: source.scope,
    };

    for (state, expected) in [
        (Availability::Pending, Availability::Pending),
        (Availability::Absent, Availability::Absent),
    ] {
        let actual = input(state)
            .try_then(|_| -> Result<Answer<Owned<&str>>, Infallible> {
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

    for (state, expected) in [
        (Availability::Pending, Availability::Pending),
        (Availability::Absent, Availability::Absent),
        (
            Availability::Value(Owned("follow-up")),
            Availability::Value(Owned("follow-up")),
        ),
    ] {
        let next = Answer {
            state,
            version: downstream.version,
            scope: downstream.scope,
        };
        let actual = input(Availability::Value(Owned(7_u32)))
            .try_then(|Owned(value)| {
                assert_eq!(value, 7);
                Ok::<_, Infallible>(next)
            })
            .unwrap();
        assert_eq!(
            actual,
            Answer {
                state: expected,
                version: downstream.version,
                scope: downstream.scope,
            }
        );
    }

    let error = Box::new(Owned("failure"));
    let identity = std::ptr::from_ref(error.as_ref());
    let actual = input(Availability::Value(Owned(7_u32)))
        .try_then(|_| Err::<Answer<Owned<()>>, _>(error))
        .unwrap_err();
    assert!(std::ptr::eq(actual.as_ref(), identity));
}
