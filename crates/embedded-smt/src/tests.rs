use super::{Bool, Bv, Context, DEFAULT_RLIMIT, Outcome, Provider, Unknown, check};
use num_bigint::BigUint;
use num_traits::ToPrimitive;

const PROVIDERS: [Provider; 3] = [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5];

fn number(outcome: Outcome) -> u64 {
    let Outcome::Sat(Some(value)) = outcome else {
        panic!("expected SAT with a model, got {outcome:?}")
    };
    let (digits, radix) = if let Some(hex) = value.strip_prefix("#x") {
        (hex, 16)
    } else {
        (
            value
                .strip_prefix("#b")
                .expect("typed SMT bit-vector literal"),
            2,
        )
    };
    BigUint::parse_bytes(digits.as_bytes(), radix)
        .unwrap()
        .to_u64()
        .unwrap()
}

#[test]
fn all_providers_keep_sat_unsat_and_resource_unknown_distinct() {
    for provider in PROVIDERS {
        let symbols = Context::default();
        let x = symbols.fresh_bv("x", 8);
        let one = Bv::from_u64(1, 8);
        assert_eq!(
            check(&[x.eq(&one)], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None)
        );
        assert_eq!(
            check(
                &[x.eq(&one), x.eq(&one).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat
        );
        let condition = x.bvmul(&x).eq(Bv::from_u64(4, 8));
        assert_eq!(
            check(std::slice::from_ref(&condition), None, provider, 1),
            Outcome::Unknown(Unknown::ResourceLimit),
            "{provider}"
        );
        assert!(
            matches!(
                check(&[condition], None, provider, DEFAULT_RLIMIT),
                Outcome::Sat(_)
            ),
            "exhaustion leaked into {provider}'s next check"
        );
        assert!(matches!(check(&[], None, provider, 0), Outcome::Error(_)));
    }
}

#[test]
fn independent_symbol_factories_do_not_alias_but_cloned_terms_do() {
    let x = Context::default().fresh_bv("same", 256);
    let y = Context::default().fresh_bv("same", 256);
    let assertions = [x.eq(Bv::from_u64(1, 256)), y.eq(Bv::from_u64(2, 256))];
    for provider in PROVIDERS {
        assert_eq!(
            check(&assertions, None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None),
            "{provider}"
        );
        assert_eq!(
            check(&[x.eq(x.clone()).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
    }
}

#[test]
fn provider_primitives_match_small_integer_oracles_including_zero_and_sign_edges() {
    for (a, b) in [(0u8, 0u8), (7, 3), (128, 255), (255, 8)] {
        let symbols = Context::default();
        let x = symbols.fresh_bv("x", 8);
        let y = symbols.fresh_bv("y", 8);
        let guards = [
            x.eq(Bv::from_u64(a.into(), 8)),
            y.eq(Bv::from_u64(b.into(), 8)),
        ];
        let signed_a = i16::from(a as i8);
        let signed_b = i16::from(b as i8);
        let sdiv = if b == 0 {
            if signed_a < 0 { 1 } else { 255 }
        } else {
            (signed_a / signed_b) as u8
        };
        let srem = if b == 0 {
            a
        } else {
            (signed_a % signed_b) as u8
        };
        let cases = [
            (x.bvadd(&y), a.wrapping_add(b)),
            (x.bvsub(&y), a.wrapping_sub(b)),
            (x.bvmul(&y), a.wrapping_mul(b)),
            (x.bvudiv(&y), a.checked_div(b).unwrap_or(255)),
            (x.bvurem(&y), a.checked_rem(b).unwrap_or(a)),
            (x.bvsdiv(&y), sdiv),
            (x.bvsrem(&y), srem),
            (x.bvand(&y), a & b),
            (x.bvor(&y), a | b),
            (x.bvxor(&y), a ^ b),
            (x.bvnot(), !a),
            (x.bvshl(&y), a.checked_shl(b.into()).unwrap_or(0)),
            (x.bvlshr(&y), a.checked_shr(b.into()).unwrap_or(0)),
            (
                x.bvashr(&y),
                (a as i8)
                    .checked_shr(b.into())
                    .unwrap_or(if signed_a < 0 { -1 } else { 0 }) as u8,
            ),
            (x.extract(7, 4), a >> 4),
        ];
        let comparisons = [
            (x.eq(&y), a == b),
            (x.bvult(&y), a < b),
            (x.bvule(&y), a <= b),
            (x.bvugt(&y), a > b),
            (x.bvuge(&y), a >= b),
            (x.bvslt(&y), signed_a < signed_b),
            (x.bvsle(&y), signed_a <= signed_b),
            (x.bvsgt(&y), signed_a > signed_b),
            (x.bvsge(&y), signed_a >= signed_b),
        ];
        for provider in PROVIDERS {
            for (term, expected) in &cases {
                assert_eq!(
                    number(check(&guards, Some(term), provider, DEFAULT_RLIMIT)),
                    u64::from(*expected),
                    "{provider}: a={a}, b={b}, term={term:?}"
                );
            }
            for (condition, expected) in &comparisons {
                let term = condition.ite(&Bv::from_u64(1, 8), &Bv::from_u64(0, 8));
                assert_eq!(
                    number(check(&guards, Some(&term), provider, DEFAULT_RLIMIT)),
                    u64::from(*expected),
                    "{provider}: a={a}, b={b}"
                );
            }
            assert_eq!(
                number(check(
                    &guards,
                    Some(&x.zero_ext(248)),
                    provider,
                    DEFAULT_RLIMIT
                )),
                u64::from(a)
            );
            let tautology = Bool::or(&[x.eq(&y), x.eq(&y).not()]);
            assert_eq!(
                check(
                    &[Bool::and(&[tautology, Bool::and(&[])])],
                    None,
                    provider,
                    DEFAULT_RLIMIT
                ),
                Outcome::Sat(None)
            );
            assert_eq!(
                check(&[Bool::or(&[])], None, provider, DEFAULT_RLIMIT),
                Outcome::Unsat
            );
        }
    }
}
