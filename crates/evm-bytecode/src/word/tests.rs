use super::{WordError, arity, evaluate, test_support::EVALUATIONS};
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[test]
fn every_opcode_and_wrong_arity_is_checked_before_numeric_work() {
    let _scope = EVALUATIONS.scope();
    for op in u8::MIN..=u8::MAX {
        for count in 0..=4 {
            EVALUATIONS.set((0, 0));
            let result = evaluate(op, &vec![U256::ZERO; count]);
            match arity(op) {
                None => assert_eq!(result, Err(WordError::UnsupportedOpcode { opcode: op })),
                Some(expected) if count != expected => assert_eq!(
                    result,
                    Err(WordError::OperandCount {
                        opcode: op,
                        expected,
                        actual: count,
                    })
                ),
                Some(_) => {
                    assert!(result.is_ok(), "supported opcode {op:#04x}");
                    assert_eq!(EVALUATIONS.get().0, 1);
                    continue;
                }
            }
            assert_eq!(EVALUATIONS.get(), (0, 0));
        }
    }
}

#[test]
fn pop_order_and_full_width_modular_intermediates_match_evm_rules() {
    let word: fn(u64) -> U256 = U256::from;
    for (op, operands, expected) in [
        (opcode::SUB, vec![word(2), word(3)], U256::MAX),
        (opcode::DIV, vec![word(7), word(2)], word(3)),
        (opcode::EXP, vec![word(2), word(256)], U256::ZERO),
        (opcode::ADDMOD, vec![U256::MAX, U256::ONE, word(7)], word(2)),
        (
            opcode::MULMOD,
            vec![U256::MAX, U256::MAX, word(7)],
            U256::ONE,
        ),
        (opcode::SHL, vec![word(8), U256::ONE], word(256)),
        (opcode::SHR, vec![word(8), word(256)], U256::ONE),
        (opcode::BYTE, vec![word(31), word(0xabcd)], word(0xcd)),
        (opcode::BYTE, vec![word(30), word(0xabcd)], word(0xab)),
        (
            opcode::SIGNEXTEND,
            vec![U256::ZERO, word(0x80)],
            U256::MAX - word(127),
        ),
    ] {
        assert_eq!(evaluate(op, &operands), Ok(expected), "opcode {op:#04x}");
    }
}

#[test]
fn signed_extremes_zero_divisors_and_large_indices_do_not_panic() {
    let minimum = U256::ONE << 255;
    let negative_three = U256::MAX - U256::from(2);
    assert_eq!(evaluate(opcode::SDIV, &[minimum, U256::MAX]), Ok(minimum));
    assert_eq!(
        evaluate(opcode::SDIV, &[negative_three, U256::from(2)]),
        Ok(U256::MAX)
    );
    assert_eq!(
        evaluate(opcode::SMOD, &[negative_three, U256::from(2)]),
        Ok(U256::MAX)
    );
    for op in [opcode::DIV, opcode::SDIV, opcode::MOD, opcode::SMOD] {
        assert_eq!(evaluate(op, &[minimum, U256::ZERO]), Ok(U256::ZERO));
    }
    for op in [opcode::ADDMOD, opcode::MULMOD] {
        assert_eq!(
            evaluate(op, &[U256::MAX, U256::MAX, U256::ZERO]),
            Ok(U256::ZERO)
        );
    }
    for amount in [U256::from(256), U256::MAX] {
        for op in [opcode::SHL, opcode::SHR] {
            assert_eq!(evaluate(op, &[amount, U256::MAX]), Ok(U256::ZERO));
        }
        assert_eq!(evaluate(opcode::SAR, &[amount, minimum]), Ok(U256::MAX));
        assert_eq!(evaluate(opcode::SAR, &[amount, U256::ONE]), Ok(U256::ZERO));
        assert_eq!(evaluate(opcode::BYTE, &[amount, U256::MAX]), Ok(U256::ZERO));
        assert_eq!(
            evaluate(opcode::SIGNEXTEND, &[amount, minimum]),
            Ok(minimum)
        );
    }
    assert_eq!(evaluate(opcode::CLZ, &[U256::ZERO]), Ok(U256::from(256)));
    assert_eq!(evaluate(opcode::CLZ, &[minimum]), Ok(U256::ZERO));
}
