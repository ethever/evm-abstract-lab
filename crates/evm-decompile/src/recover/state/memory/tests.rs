use super::*;

fn observed_memory() -> Memory {
    let mut memory = Memory::new();
    for offset in [0x80, 0x180] {
        memory.write(
            Some(Address::Exact(U256::from(offset))),
            32,
            Some(AbstractValue::constant(U256::from(offset))),
        );
    }
    memory
}

#[test]
fn unknown_width_only_protects_cells_below_every_possible_start() {
    let mut memory = observed_memory();
    memory.write_envelope(None, (U256::from(0x100), U256::from(0x120)), None, None);
    assert_eq!(
        memory
            .read(Some(&Address::Exact(U256::from(0x80))))
            .singleton(),
        Some(U256::from(0x80))
    );
    assert!(
        memory
            .read(Some(&Address::Exact(U256::from(0x180))))
            .constants()
            .is_none()
    );
}

#[test]
fn an_overflowing_endpoint_never_becomes_a_wrapped_upper_exclusion() {
    let mut memory = observed_memory();
    memory.write_envelope(None, (U256::ZERO, U256::MAX), Some(32), None);
    assert!(
        memory
            .read(Some(&Address::Exact(U256::from(0x80))))
            .constants()
            .is_none()
    );
    let mut memory = observed_memory();
    memory.write_envelope(
        None,
        (U256::MAX - U256::from(16), U256::MAX),
        Some(32),
        None,
    );
    assert_eq!(
        memory
            .read(Some(&Address::Exact(U256::from(0x80))))
            .singleton(),
        Some(U256::from(0x80))
    );
}

#[test]
fn a_zero_width_preserves_unknown_addresses_and_implicit_zero_backing() {
    let mut memory = Memory::new();
    memory.write_envelope(None, (U256::ZERO, U256::MAX), Some(0), None);
    assert_eq!(memory.read(None).singleton(), Some(U256::ZERO));
}
