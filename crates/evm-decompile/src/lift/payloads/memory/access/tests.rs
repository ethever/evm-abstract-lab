use super::*;
use crate::lift::payloads::{memory::Identity, overlap};
use crate::{Config, ExecutionDomain, Fork, Input, decompile};
use std::cell::Cell;

fn opaque(offset: U256) -> Word {
    Word {
        base: Some(Identity::Value(EntityId(7))),
        offset,
        low: U256::ZERO,
        high: U256::MAX,
    }
}

#[test]
fn a_request_bound_is_local_and_intersects_the_source_interval() {
    let source = opaque(U256::from(28));
    let end = U256::from(1024);
    let range = AccessRange::new(source, 68, Some(end)).unwrap();
    assert_eq!(range.base.base, source.base);
    assert_eq!(range.base.offset, source.offset);
    assert_eq!(range.base.high, end - U256::from(68));
    assert_eq!(source.high, U256::MAX);
    assert!(AccessRange::new(source, 68, None).is_none());
    assert!(AccessRange::new(Word::constant(end - U256::from(67)), 68, Some(end)).is_none());
    assert!(AccessRange::new(Word::constant(end - U256::from(68)), 68, Some(end)).is_some());
    assert!(AccessRange::new(Word::constant(U256::MAX), 1, Some(end)).is_none());
    assert!(AccessRange::new(Word::constant(U256::MAX), 1, None).is_none());
}

#[test]
fn zero_length_never_constrains_its_ignored_pointer() {
    for end in [None, Some(U256::ZERO)] {
        let range = AccessRange::new(opaque(U256::MAX), 0, end).unwrap();
        assert_eq!(range.size, 0);
        assert_eq!(range.base.exact(), Some(U256::ZERO));
    }
}

#[test]
fn modular_start_arithmetic_does_not_change_ordinary_request_intersection() {
    // P=16 gives starts 8 and 24: the first ADD wraps, while both actual
    // memory requests are ordinary. Exact relative identity remains usable.
    let write = AccessRange::new(
        opaque(U256::MAX - U256::from(7)),
        32,
        Some(U256::from(1024)),
    )
    .unwrap();
    let read = AccessRange::new(opaque(U256::from(8)), 32, Some(U256::from(1024))).unwrap();
    assert_eq!(
        overlap(read.base, read.size, write.base, write.size),
        Some((0, 16, 16))
    );
    let other = Word {
        base: Some(Identity::Value(EntityId(8))),
        ..write.base
    };
    assert_eq!(overlap(read.base, read.size, other, write.size), None);
}

#[test]
fn scoped_request_evidence_never_refines_a_word_cache_or_bypasses_refusal() {
    let input = Input::new(
        vec![
            0x5f, 0x35, 0x80, 0x60, 0x42, 0x90, 0x52, 0x60, 0x20, 0x90, 0xfd,
        ],
        Fork::Osaka,
    );
    let config = Config {
        execution_domain: ExecutionDomain::EthereumTransactions,
        ..Config::default()
    };
    let artifact = decompile(&input, &config);
    let ir = &artifact.current().unwrap().ir;
    let terminal = ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0xfd)
        .unwrap();
    let allow = Cell::new(true);
    let mut memory = Memory::new(ir, |_| allow.get()).unwrap();
    let before = memory.word(terminal.inputs[0]).unwrap();
    assert_eq!(before.high, U256::MAX);
    let request = memory.terminal_access(terminal.id).unwrap();
    assert!(request.base.high < before.high);
    assert_eq!(memory.word(terminal.inputs[0]), Some(before));
    allow.set(false);
    assert!(memory.terminal_access(terminal.id).is_none());
    assert!(memory.refused);
}
