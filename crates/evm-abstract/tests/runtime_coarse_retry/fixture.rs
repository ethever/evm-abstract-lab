//! Cached graph metrics select one fixed policy, before either analysis path.

use super::support::{Assembly, Fixture, NODE_CAP, word};
use evm_abstract::{
    U256,
    domain::{
        Domain,
        symbolic::{ByteStateKind, StateError},
    },
    world::{ByteArray, RangeError},
};
use revm_bytecode::opcode;

fn bit(a: &mut Assembly) -> &mut Assembly {
    a.push(0).op(opcode::CALLDATALOAD).push(1).op(opcode::AND)
}

pub(super) fn recoverable() -> Fixture {
    let mut a = Assembly::default();
    bit(&mut a)
        .push(0)
        .op(opcode::MSTORE8)
        .jump("update", false);
    a.label("update")
        .store(opcode::SSTORE, 0, 99)
        .store(opcode::TSTORE, 0, 42)
        .push(99)
        .push(64)
        .mark("refused")
        .op(opcode::MSTORE)
        .store(opcode::SSTORE, 0, 42)
        .store(opcode::TSTORE, 0, 99)
        .push(42)
        .push(64)
        .op(opcode::MSTORE)
        .exit(opcode::RETURN, 64);
    a.finish()
}

/// A finite, ordinary implementation witnessing one callback of the envelope.
pub(super) fn concrete_environment(bit: u16) -> Fixture {
    let mut a = Assembly::default();
    a.push(bit)
        .push(0)
        .op(opcode::MSTORE)
        .push(32)
        .push(0)
        .push(32)
        .push(0)
        .push(0)
        .push(super::support::B as u16)
        .push(u16::MAX)
        .op(opcode::CALL)
        .mark("resumed")
        .op(opcode::POP)
        .exit(opcode::RETURN, 0);
    a.finish()
}

fn input_bit(domain: Domain) -> evm_abstract::domain::AbstractValue {
    // The relative callback's summary independently instantiates its calldata.
    let calldata = ByteArray::unknown()
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    let input = calldata.read_word_checked(&word(0), domain).unwrap();
    let bit = domain.apply(opcode::AND, &[word(1), input]);
    assert_eq!(bit.expression().unwrap().nodes(), 9);
    assert!(!bit.symbolic_limit_reached());
    bit
}

fn clean(array: &ByteArray, domain: Domain) {
    assert!(!array.len().symbolic_limit_reached());
    assert!(!array.default_byte().symbolic_limit_reached());
    assert!(
        array
            .stored_bytes()
            .all(|(_, value)| !value.symbolic_limit_reached())
    );
    assert!(
        array
            .symbolic_facts()
            .all(|(_, value)| !value.symbolic_limit_reached())
    );
    for index in 0..32 {
        assert!(
            !array
                .read_byte_checked(&word(index), domain)
                .unwrap()
                .symbolic_limit_reached()
        );
    }
}

pub(super) fn entry_constructor_refusal(domain: Domain) {
    assert_eq!(domain.spec().relations().max_nodes, NODE_CAP);
    let mut prefix = ByteArray::memory();
    prefix
        .write_byte(&word(0), &input_bit(domain), 65_536, domain)
        .unwrap();
    assert_eq!(prefix.len().singleton(), Some(U256::from(32)));
    clean(&prefix, domain);
    let mut entry = prefix
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    let ByteStateKind::Slice { source } = entry.state_root().kind() else {
        panic!("bounded cell summary");
    };
    let ByteStateKind::Copy {
        previous,
        destination,
        source: chunk,
        length,
    } = source.state.kind()
    else {
        panic!("constant tail overlay");
    };
    let ByteStateKind::Input { bindings, .. } = previous.kind() else {
        panic!("one observed bit");
    };
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].numeric.work_size(), 13);
    assert_eq!(previous.nodes(), 17);
    assert_eq!(destination.as_constant(), Some(U256::from(1)));
    assert_eq!(length.as_constant(), Some(U256::from(31)));
    assert!(matches!(chunk.state.kind(), ByteStateKind::Exact(bytes) if bytes.as_ref() == [0; 31]));
    assert_eq!(source.state.nodes(), 23);
    assert_eq!(entry.state_root().nodes(), 26);
    clean(&entry, domain);
    let before = entry.clone();
    assert!(
        matches!(entry.write_word(&word(64), &word(99), 65_536, domain),
        Err(RangeError::State(StateError::Limit { nodes: 29, depth })) if depth <= 64)
    );
    assert_eq!(entry, before, "refused constructor is atomic");
}
