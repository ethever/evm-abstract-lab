use super::super::ByteArray;
use crate::domain::{AbstractValue, Domain, symbolic::ExprId};
use alloy_primitives::U256;

fn value(value: U256) -> AbstractValue {
    AbstractValue::constant(value)
}

#[test]
fn huge_copy_slice_and_write_retain_u256_lengths_without_materializing() {
    let domain = Domain::default();
    let size = value(U256::from(1) << 128usize);
    let at = value(U256::from(1) << 160usize);
    let source = ByteArray::unknown();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(&at, &source, &value(U256::ZERO), &size, 64, domain)
        .unwrap();
    assert_eq!(
        memory.len().singleton(),
        at.singleton()
            .unwrap()
            .checked_add(size.singleton().unwrap())
    );
    let sliced = memory.slice(&at, &size, 64, domain).unwrap();
    assert_eq!(sliced.len().singleton(), size.singleton());
    assert_eq!(
        sliced.state_root(),
        source
            .slice(&value(U256::ZERO), &size, 64, domain)
            .unwrap()
            .state_root()
    );
    assert_eq!(memory.stored_bytes().len(), 0);
    assert!(memory.work_size() < 4096);
    assert!(sliced.work_size() < 4096);
    assert!(sliced.exact_bytes_bounded(64).is_none());
    memory
        .write_byte(&at, &value(U256::from(0xab)), 64, domain)
        .unwrap();
    assert_eq!(
        memory.read_byte_checked(&at, domain).unwrap().singleton(),
        Some(U256::from(0xab))
    );
    assert!(memory.stored_bytes().len() <= 256);
}

#[test]
fn word_rounded_bound_is_exact_and_zero_at_max_is_unchanged() {
    let domain = Domain::default();
    let bound = U256::MAX & !U256::from(31);
    let mut memory = ByteArray::memory();
    memory
        .expand(
            &value(bound - U256::from(32)),
            &value(U256::from(32)),
            1,
            domain,
        )
        .unwrap();
    assert_eq!(memory.len().singleton(), Some(bound));
    let before = memory.clone();
    memory
        .expand(&value(U256::MAX), &value(U256::ZERO), 1, domain)
        .unwrap();
    assert_eq!(memory, before);
    assert!(
        memory
            .expand(&value(bound), &value(U256::from(1)), 1, domain)
            .is_err()
    );
    assert_eq!(memory, before);
    assert!(
        memory
            .expand(&value(U256::ZERO), &value(bound + U256::from(1)), 1, domain)
            .is_err()
    );
    assert_eq!(memory, before);
}

#[test]
fn possible_symbolic_overflow_uses_a_stable_coarse_length_not_wrapped_add() {
    let domain = Domain::default();
    let offset = AbstractValue::unsigned_range(U256::MAX - U256::from(64), U256::MAX)
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let size = value(U256::from(32));
    let mut memory = ByteArray::memory();
    memory
        .expand(&value(U256::ZERO), &value(U256::from(96)), 1, domain)
        .unwrap();
    memory.expand(&offset, &size, 1, domain).unwrap();
    let length = memory.len().clone();
    assert_eq!(
        length.interval().unsigned_bounds(),
        (U256::from(96), U256::MAX)
    );
    assert!(length.singleton().is_none());
    assert!(!matches!(
        length.expression().unwrap().kind(),
        crate::domain::symbolic::ExprKind::Operation { .. }
    ));
    for _ in 0..4 {
        memory.expand(&offset, &size, 1, domain).unwrap();
        assert_eq!(memory.len(), &length);
    }
}

#[test]
fn numeric_only_copy_uses_the_same_synthesized_operand_in_root_and_allocation() {
    use crate::domain::symbolic::ByteStateKind;
    let domain = Domain::default();
    let offset = domain.join(&value(U256::from(64)), &value(U256::from(96)));
    let size = domain.join(&value(U256::from(1)), &value(U256::from(2)));
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &offset,
            &ByteArray::exact(&[1, 2]),
            &value(U256::ZERO),
            &size,
            1,
            domain,
        )
        .unwrap();
    let ByteStateKind::Copy {
        destination,
        length,
        source,
        ..
    } = memory.state_root().kind()
    else {
        panic!("retain the full lazy copy");
    };
    assert!(
        memory
            .allocation_facts()
            .any(|(at, size)| at == destination && size == length)
    );
    assert_eq!(&source.length, length);
}
