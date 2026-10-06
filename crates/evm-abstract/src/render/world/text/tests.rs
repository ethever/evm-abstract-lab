use super::{render, write_table};
use crate::{
    Fork,
    analysis::{ExecutionConfig, analyze_world},
    domain::Value,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};

#[test]
fn wrapping_keeps_every_character_in_a_long_abstract_value() {
    let value = format!("{{{}}}", "⊤abc0123".repeat(30));
    let mut output = String::new();
    write_table(&mut output, "", &["Value"], &[vec![value.clone()]]);
    let reconstructed = output
        .lines()
        .skip(2)
        .map(str::trim_end)
        .collect::<String>();
    assert_eq!(reconstructed, value);
    assert!(output.lines().all(|line| line.chars().count() <= 72));
}

#[test]
fn a_store_with_unobserved_logs_keeps_the_unknown_flag_visible() {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "unknown logs");
    world.insert(address, Account::empty()).unwrap();
    let mut analysis = analyze_world(
        world,
        Entry {
            address,
            caller: Address::ZERO,
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
        },
        ExecutionConfig::default(),
    )
    .unwrap();
    analysis.outcomes[0].store.havoc_all();
    let output = render(&analysis);
    assert!(output.contains("logs_unknown=true"));
    assert_eq!(output.matches("logs_unknown=true").count(), 1);
}
