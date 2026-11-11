//! Explicit resource premises, fixed-cell joins and changing loop contents.
use super::*;
use evm_decompile::{Completion, ExecutionDomain, Input, ValidationStatus, validate_stage};

const PAIR_ERROR: &[u8] = b"UniswapV2: TRANSFER_FAILED";

fn compact_pair() -> Input {
    Input::new(
        evm_bytecode::parse_hex(include_str!(
            "../../fixtures/error-payloads/pair-returndata-allocation-guard.hex"
        ))
        .unwrap(),
        evm_decompile::Fork::Osaka,
    )
}

fn bounded(input: &Input) -> Artifact {
    let artifact = decompile(
        input,
        &Config {
            execution_domain: ExecutionDomain::EthereumTransactions,
            ..Config::default()
        },
    );
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    super::super::support::validated(&artifact);
    artifact
}

#[test]
fn actual_pair_constructor_after_return_allocation_requires_the_declared_domain() {
    let input = compact_pair();
    let original = evm_bytecode::parse_hex(include_str!(
        "../../fixtures/structure/ethereum-uniswap-v2-usdc-weth-pair.hex"
    ))
    .unwrap();
    assert_eq!(input.runtime_bytecode.len(), 190);
    assert_eq!(&input.runtime_bytecode[88..], &original[8571..8673]);
    let generic = checked(&input);
    assert!(lifted(&generic).1.reverts.is_empty());
    let ethereum = bounded(&input);
    assert_message(&ethereum, PAIR_ERROR);
    assert_eq!(lifted(&ethereum).1.reverts.len(), 1);
    for artifact in [&generic, &ethereum] {
        for length in [0, 1, 31, 32, 96] {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(ExternalResponse {
                result: U256::from(1),
                return_data: vec![0xee; length],
                ..ExternalResponse::default()
            });
            let (outcome, observed, capsules) = oracle::certify(&input, artifact, host, 1_048_576);
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &observed),
                Some(oracle::abi(PAIR_ERROR))
            );
            assert_eq!(
                capsules,
                usize::from(
                    artifact.config.execution_domain == ExecutionDomain::EthereumTransactions
                )
            );
        }
    }
    let mut wrong = ethereum.current().unwrap().clone();
    wrong.ir.execution_domain = ExecutionDomain::Generic;
    wrong.ir.execution_fork = None;
    assert!(
        wrong
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .validate_guard_sources(&wrong.ir)
            .is_err()
    );
    assert_ne!(
        validate_stage(&wrong).status,
        ValidationStatus::Passed,
        "a bounded proof cannot become a generic certificate by erasing its premise"
    );
    // Even a matching forged scope marker cannot replace semantic source replay.
    for proof in &mut wrong.ir.lifted.as_mut().unwrap().reverts {
        proof.execution_domain = ExecutionDomain::Generic;
        proof.execution_fork = None;
    }
    wrong
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .validate_guard_sources(&wrong.ir)
        .unwrap();
    let validation = validate_stage(&wrong);
    assert_ne!(validation.status, ValidationStatus::Passed);
    assert!(
        validation.checks.iter().any(|check| !check.passed
            && check.name == "lifted_semantic_projection"
            && check.message.contains("code/memory payload")),
        "scope-erased claims must fail independent numeric replay: {validation:?}"
    );
}

fn changing_entry_pointer(second: u64) -> Input {
    let mut asm = Asm::default();
    // n is bounded by source arithmetic; each entry chooses its own initial p.
    asm.number(32).op(0x35).number(3).op(0x16);
    asm.number(0).op(0x35).target("second").op(0x57);
    initialize(&mut asm, 128);
    asm.target("loop").op(0x56).dest("second");
    initialize(&mut asm, u16::try_from(second).unwrap());
    asm.dest("loop").op(0x80).op(0x15).target("exit").op(0x57);
    asm.number(64)
        .op(0x51)
        .number(32)
        .op(0x01)
        .number(64)
        .op(0x52);
    asm.number(1).op(0x90).op(0x03).target("loop").op(0x56);
    asm.dest("exit").op(0x50);
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

#[test]
fn two_external_entries_do_not_make_a_changing_cell_iteration_invariant() {
    let input = changing_entry_pointer(160);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    for entry in [0, 1_u64] {
        for n in 0..=3_u64 {
            let (outcome, observed, capsules) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(entry), U256::from(n)])),
                1_048_576,
            );
            let outcome = outcome.unwrap();
            let expected = U256::from(if entry == 0 { 128 } else { 160 } + n * 32);
            assert_eq!(
                oracle::reverted(&outcome, &observed),
                Some(oracle::abi(DS_MATH))
            );
            let ExecutionOutcome::Exit { values, .. } = outcome else {
                unreachable!()
            };
            assert_eq!(values, vec![expected, U256::from(100)]);
            assert_eq!(U256::from_be_slice(&observed.memory[64..96]), expected);
            assert_eq!(capsules, 1);
        }
    }
    let mut wrong = artifact.current().unwrap().clone();
    let counter = wrong
        .ir
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|op| op.opcode == 0x03)
        .unwrap();
    counter.opcode = 0x01;
    assert_ne!(
        validate_stage(&wrong).status,
        ValidationStatus::Passed,
        "a changed counter must be replayed rather than importing a previous loop proof"
    );
}

#[test]
fn one_unsafe_external_entry_still_prevents_a_payload_certificate() {
    let input = changing_entry_pointer(64);
    for artifact in [checked(&input), bounded(&input)] {
        assert!(
            lifted(&artifact).1.reverts.is_empty(),
            "one entry can place the constructor on its free-pointer cell"
        );
    }
}

#[test]
fn a_transaction_premise_does_not_bound_arbitrary_calldata_allocator_words() {
    let mut asm = Asm::default();
    asm.number(0)
        .op(0x35)
        .number(63)
        .op(0x01)
        .word(!U256::from(31))
        .op(0x16);
    asm.number(292).op(0x01).number(64).op(0x52);
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    let input = asm.finish();
    // The masked allocation wraps to4. A positive pointer alone cannot justify
    // disjointness from the actual free-pointer cell or exact terminal bytes.
    let value = U256::MAX - U256::from(324);
    let pointer =
        U256::from(292).wrapping_add(value.wrapping_add(U256::from(63)) & !U256::from(31));
    assert_eq!(pointer, U256::from(4));
    for artifact in [checked(&input), bounded(&input)] {
        assert!(lifted(&artifact).1.reverts.is_empty());
        let (outcome, _, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[value])),
            1_048_576,
        );
        assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
        assert_eq!(capsules, 0);
    }
}

#[derive(Clone, Copy, Debug)]
enum SourceBound {
    Upper,
    Flipped,
    Missing,
}

fn source_bounded_return_size(bound: SourceBound) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    for _ in 0..5 {
        asm.number(0);
    }
    asm.number(0x42).number(100_000).op(0xf1).op(0x50).op(0x3d);
    // Keep this exact RETURNDATASIZE word through the guard and both branches.
    // No relation between later return-data observations is assumed.
    if !matches!(bound, SourceBound::Missing) {
        asm.op(0x80).number(255).op(0x90);
        asm.op(if matches!(bound, SourceBound::Upper) {
            0x11
        } else {
            0x10
        });
        asm.target("reject").op(0x57);
    }
    asm.op(0x80).op(0x15).target("after").op(0x57);
    asm.number(64).op(0x51).op(0x81).number(63).op(0x01);
    asm.word(!U256::from(31))
        .op(0x16)
        .op(0x81)
        .op(0x01)
        .number(64)
        .op(0x52);
    asm.op(0x81).op(0x81).op(0x52);
    asm.op(0x81)
        .number(0)
        .op(0x82)
        .number(32)
        .op(0x01)
        .op(0x3e)
        .op(0x50);
    asm.dest("after").op(0x50);
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.dest("reject").number(0).number(0).op(0xfd);
    asm.finish()
}

#[test]
fn generic_source_bounds_on_the_same_saved_return_size_close_the_payload() {
    for bound in [
        SourceBound::Upper,
        SourceBound::Flipped,
        SourceBound::Missing,
    ] {
        let input = source_bounded_return_size(bound);
        let artifact = checked(&input);
        if matches!(bound, SourceBound::Upper) {
            assert_message(&artifact, DS_MATH);
        } else {
            assert!(
                lifted(&artifact).1.reverts.is_empty(),
                "the accepted path has no upper size bound: {bound:?}"
            );
        }
        for length in [0, 1, 31, 255, 256] {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(ExternalResponse {
                result: U256::from(1),
                return_data: vec![0xee; length],
                ..ExternalResponse::default()
            });
            let (outcome, observed, _) = oracle::certify(&input, &artifact, host, 1_048_576);
            let rejected = match bound {
                SourceBound::Upper => length > 255,
                SourceBound::Flipped => length < 255,
                SourceBound::Missing => false,
            };
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &observed),
                Some(if rejected {
                    Vec::new()
                } else {
                    oracle::abi(DS_MATH)
                })
            );
        }
    }
}
