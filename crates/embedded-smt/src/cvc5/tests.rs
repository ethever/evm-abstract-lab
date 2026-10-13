use crate::{
    Array, Bv, Cancellation, Context, DEFAULT_RLIMIT, Outcome, Provider, Unknown, check,
    with_cancellation,
};
use num_bigint::BigUint;

#[test]
fn model_preserves_a_wide_non_hex_aligned_bit_vector() {
    let number = (BigUint::from(1_u8) << 256_usize) + BigUint::from(1_u8);
    let value = Bv::from_str(257, &number.to_string()).unwrap();
    let Outcome::Sat(Some(model)) = check(&[], Some(&value), Provider::Cvc5, DEFAULT_RLIMIT) else {
        panic!("a constant must have a model within the default allowance");
    };
    let bits = model.strip_prefix("#b").unwrap();
    assert_eq!(bits.len(), 257);
    assert_eq!(BigUint::parse_bytes(bits.as_bytes(), 2).unwrap(), number);
}

#[test]
fn exhausted_allowance_is_typed_and_does_not_leak_into_the_next_session() {
    let context = Context::default();
    let input = context.fresh_bv("square_root", 8);
    let assertion = input.bvmul(&input).eq(Bv::from_u64(4, 8));
    assert_eq!(
        check(std::slice::from_ref(&assertion), None, Provider::Cvc5, 1),
        Outcome::Unknown(Unknown::ResourceLimit),
    );

    let Outcome::Sat(Some(model)) =
        check(&[assertion], Some(&input), Provider::Cvc5, DEFAULT_RLIMIT)
    else {
        panic!("a fresh session must receive its own resource allowance");
    };
    let root = u16::from_str_radix(model.strip_prefix("#b").unwrap(), 2).unwrap();
    assert_eq!((root * root) & 0xff, 4);
}

#[test]
fn repeated_square_auxiliary_equations_keep_their_dag_under_resource_exhaustion() {
    let context = Context::default();
    let input = context.fresh_bv("square_base", 256);
    let mut value = input;
    let mut assertions = Vec::new();
    // Inlining these exact equations produces an exponentially long product.
    // Native preprocessing must retain their bounded shared representation.
    for _ in 0..256 {
        let next = context.fresh_bv("square_auxiliary", 256);
        assertions.push(next.eq(value.bvmul(&value)));
        value = next;
    }
    assert_eq!(
        check(&assertions, Some(&value), Provider::Cvc5, 1),
        Outcome::Unknown(Unknown::ResourceLimit),
    );
}

#[test]
fn auxiliary_definitions_and_root_guards_still_constrain_models() {
    // 256-bit multiplications cross the native abstraction threshold. Both the
    // model and the contradiction must be checked against the exact equations.
    for width in [8, 256] {
        let context = Context::default();
        let input = context.fresh_bv("base", width);
        let square = context.fresh_bv("square", width);
        let fourth = context.fresh_bv("fourth", width);
        let mut assertions = vec![
            input.eq(Bv::from_u64(3, width)),
            square.eq(input.bvmul(&input)),
            fourth.eq(square.bvmul(&square)),
        ];
        let Outcome::Sat(Some(model)) =
            check(&assertions, Some(&fourth), Provider::Cvc5, DEFAULT_RLIMIT)
        else {
            panic!("the auxiliary equations must admit the exact fourth power");
        };
        let bits = model.strip_prefix("#b").unwrap();
        assert_eq!(bits.len(), width as usize);
        assert_eq!(
            BigUint::parse_bytes(bits.as_bytes(), 2).unwrap(),
            BigUint::from(81_u8)
        );
        assertions.push(fourth.eq(Bv::from_u64(80, width)));
        assert_eq!(
            check(&assertions, Some(&fourth), Provider::Cvc5, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "width={width}",
        );
    }
}

#[test]
fn cancellation_during_native_array_check_discards_the_answer_after_native_return() {
    use super::{Backend, Cvc5, Error, Failure, Native, Node, Status, backend};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    // Only this test observer sends the entry signal. All native work still
    // runs on this thread, using the production adapter and encoding.
    struct ObserveCheck {
        inner: Cvc5,
        entered: mpsc::Sender<()>,
    }
    impl Backend for ObserveCheck {
        type Term = Native;
        fn make(&mut self, node: &Node, args: &[Native]) -> Result<Native, Failure> {
            self.inner.make(node, args)
        }
        fn assert(&mut self, term: &Native) -> Result<(), Error> {
            self.inner.assert(term)
        }
        fn check(&mut self) -> Result<Status, Error> {
            self.entered.send(()).unwrap();
            let result = self.inner.check();
            assert!(matches!(
                &result,
                Ok(Status::Unknown(Unknown::ResourceLimit))
            ));
            result
        }
        fn model(&mut self, term: &Native) -> Result<String, Error> {
            self.inner.model(term)
        }
    }

    let context = Context::default();
    let default = context.fresh_bv("default", 13);
    let array = context.fresh_array("array", 256, 13);
    let index = context.fresh_bv("index", 256);
    let assertions = [
        Array::constant(256, &default).eq(&array),
        array.select(&index).eq(&default).not(),
    ];
    let allowance = 1_000_000;
    let token = Cancellation::new();
    let cancel_token = token.clone();
    let (entered, receiver) = mpsc::channel();
    let (cancelled, cancellation_time) = mpsc::channel::<Instant>();
    let observer = ObserveCheck {
        inner: Cvc5::new(allowance, true).unwrap(),
        entered,
    };
    std::thread::scope(|scope| {
        scope.spawn(move || {
            receiver.recv().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            cancel_token.cancel();
            cancelled.send(Instant::now()).unwrap();
        });
        let outcome = with_cancellation(&token, || {
            backend::execute(&assertions, None, observer, allowance)
        });
        let cancelled_at = cancellation_time.recv().unwrap();
        eprintln!(
            "cvc5 active array check: rlimit={allowance}, native=ResourceLimit, public={outcome:?}, cancellation delay={:?}",
            cancelled_at.elapsed()
        );
        assert_eq!(outcome, Outcome::Unknown(Unknown::Cancelled));
    });
    assert_eq!(
        check(
            &[],
            Some(&Bv::from_u64(7, 13)),
            Provider::Cvc5,
            DEFAULT_RLIMIT
        ),
        Outcome::Sat(Some("#b0000000000111".into()))
    );
}
