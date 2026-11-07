use super::*;

#[test]
fn noncanonical_unknown_aliasing_and_invalid_utf8_payloads_stay_raw() {
    for mutation in [
        Mutation::NonzeroPadding,
        Mutation::Offset,
        Mutation::Length,
        Mutation::UnknownWord,
        Mutation::UnknownStore,
        Mutation::ByteOverlap,
        Mutation::Pointer(160),
        Mutation::UnknownCopy(0x37),
        Mutation::UnknownCopy(0x39),
        Mutation::UnknownCopy(0x5e),
        Mutation::Copy {
            opcode: 0x37,
            destination: 196,
        },
        Mutation::Copy {
            opcode: 0x39,
            destination: 64,
        },
        Mutation::CallOutput {
            destination: 196,
            size: 32,
        },
        Mutation::UnknownCallOutput,
    ] {
        let input = fixtures::standalone(DS_MATH, mutation);
        let artifact = checked(&input);
        assert!(
            lifted(&artifact).1.reverts.is_empty(),
            "accepted {mutation:?}: {}",
            render_text(&artifact)
        );
        assert!(lifted(&artifact).1.constructions.is_empty());
        let mut host = MemoryHost::new(&input, calldata(&[U256::ZERO, U256::from(196)]));
        host.responses.push_back(ExternalResponse {
            return_data: vec![0xee; 32],
            ..ExternalResponse::default()
        });
        let _ = oracle::certify(&input, &artifact, host, 1_048_576);
    }
    let input = fixtures::standalone(&[0xff, 0xfe, 0x80], Mutation::None);
    let artifact = checked(&input);
    assert!(lifted(&artifact).1.payloads.is_empty());
    let (outcome, host, _) = oracle::certify(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        1_048_576,
    );
    assert_eq!(
        oracle::reverted(&outcome.unwrap(), &host),
        Some(oracle::abi(&[0xff, 0xfe, 0x80]))
    );
}

#[test]
fn exact_pointer_updates_and_nonaliasing_writes_keep_terminal_content_proof() {
    for pointer in [128_u16, 160, 256, 512] {
        let mut asm = Asm::default();
        initialize(&mut asm, 128);
        initialize(&mut asm, pointer);
        begin(&mut asm, DS_MATH);
        end(&mut asm, DS_MATH.len());
        let input = asm.finish();
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        let _ = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            1_048_576,
        );
    }
    for mutation in [
        Mutation::Pointer(128),
        Mutation::DisjointStore,
        Mutation::Copy {
            opcode: 0x37,
            destination: 0x300,
        },
        Mutation::Copy {
            opcode: 0x39,
            destination: 0x300,
        },
        Mutation::Copy {
            opcode: 0x5e,
            destination: 0x300,
        },
        Mutation::CallOutput {
            destination: 0x300,
            size: 32,
        },
    ] {
        let input = fixtures::standalone(DS_MATH, mutation);
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(ExternalResponse::default());
        let _ = oracle::certify(&input, &artifact, host, 1_048_576);
    }
}

#[test]
fn all_predecessors_and_pointer_identity_are_required_at_the_actual_memory_version() {
    let input = fixtures::predecessor_pointer(true);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    for value in [0_u64, 1] {
        let (outcome, host, _) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(value)])),
            1_048_576,
        );
        assert_eq!(
            oracle::reverted(&outcome.unwrap(), &host),
            Some(oracle::abi(DS_MATH))
        );
    }
    for input in [
        fixtures::predecessor_pointer(false),
        fixtures::equal_ranges_distinct_pointers(),
    ] {
        let artifact = checked(&input);
        assert!(
            lifted(&artifact).1.reverts.is_empty(),
            "pointer range was used as equality"
        );
        for (p, q) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
            let _ = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(p), U256::from(q)])),
                1_048_576,
            );
        }
    }
}

#[test]
fn codecopy_padding_gap_and_forwarded_returndata_never_guess_a_string() {
    for input in [fixtures::codecopy_gap(), fixtures::forwarded()] {
        let artifact = checked(&input);
        assert!(lifted(&artifact).1.payloads.is_empty());
        assert!(lifted(&artifact).1.reverts.is_empty());
        assert!(!render_text(&artifact).contains("evm.revert(\""));
        let mut host = MemoryHost::new(&input, Vec::new());
        host.return_data = oracle::abi(DS_MATH);
        let _ = oracle::certify(&input, &artifact, host, 1_048_576);
    }
}

#[test]
fn bounded_symbolic_free_pointer_updates_and_disjoint_loops_keep_real_content_proofs() {
    for input in [
        fixtures::bounded_pointer_update(),
        fixtures::loop_pointer(false),
    ] {
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        for value in [0_u64, 1, 31, 255, u64::MAX] {
            let (outcome, host, _) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(value)])),
                1_048_576,
            );
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(DS_MATH))
            );
        }
    }
    let input = fixtures::loop_pointer(true);
    let artifact = checked(&input);
    assert!(
        lifted(&artifact).1.reverts.is_empty(),
        "loop may overwrite the actual free-pointer slot"
    );
    for value in [64_u64, 512] {
        let _ = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(value)])),
            1_048_576,
        );
    }
}

#[test]
fn wrapping_addresses_and_uninitialized_free_pointer_are_not_guessed() {
    for pointer in [None, Some(U256::MAX - U256::from(64))] {
        let mut asm = Asm::default();
        if let Some(pointer) = pointer {
            asm.word(pointer).number(64).op(0x52);
        }
        begin(&mut asm, DS_MATH);
        end(&mut asm, DS_MATH.len());
        let input = asm.finish();
        let artifact = checked(&input);
        assert!(lifted(&artifact).1.reverts.is_empty());
        let _ = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            1_048_576,
        );
    }
}

#[test]
fn all_external_call_output_ranges_are_checked_for_slot_and_payload_aliases() {
    for opcode in [0xf1, 0xf2, 0xf4, 0xfa] {
        for destination in [64_u16, 196, 0x300] {
            let input = fixtures::standalone(
                DS_MATH,
                Mutation::CallVariant {
                    opcode,
                    destination,
                },
            );
            let artifact = checked(&input);
            if destination == 0x300 {
                assert_message(&artifact, DS_MATH);
            } else {
                assert!(
                    lifted(&artifact).1.reverts.is_empty(),
                    "aliasing call {opcode:#x} at {destination}"
                );
            }
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(ExternalResponse {
                result: U256::from(1),
                return_data: vec![0xee; 32],
                ..ExternalResponse::default()
            });
            let _ = oracle::certify(&input, &artifact, host, 1_048_576);
        }
    }
}

#[test]
fn unknown_copy_width_cannot_alias_a_lower_payload_or_free_pointer_cell() {
    for opcode in [0x37, 0x39, 0x5e] {
        let input = fixtures::unknown_width_copy(opcode);
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        assert!(
            lifted(&artifact).1.constructions.is_empty(),
            "unrelated copy remains visible"
        );
        for width in [0_u64, 1, 32, 256, u64::MAX] {
            let (outcome, host, _) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(width)])),
                1_048_576,
            );
            if width == u64::MAX {
                assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
            } else {
                assert_eq!(
                    oracle::reverted(&outcome.unwrap(), &host),
                    Some(oracle::abi(DS_MATH))
                );
            }
        }
    }
}

#[test]
fn edge_conditioned_pointer_bounds_do_not_leak_into_the_other_source_arm() {
    let input = fixtures::conditional_pointer_bounds();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    let (stage, program) = lifted(&artifact);
    let evm_decompile::Terminator::Branch {
        then_edges,
        else_edge: Some(other),
        ..
    } = &stage.ir.block(stage.ir.entry.unwrap()).unwrap().terminator
    else {
        panic!("fixture entry is a comparison branch")
    };
    let bounded = stage.ir.edge(then_edges[0]).unwrap().to;
    let unbounded = stage.ir.edge(*other).unwrap().to;
    assert!(
        program.reverts.iter().any(|r| r.block == bounded),
        "x<32 proves the actual bounded arm"
    );
    assert!(
        !program.reverts.iter().any(|r| r.block == unbounded),
        "conditional no-wrap premise leaked across source arms"
    );
    for value in [
        U256::ZERO,
        U256::from(1),
        U256::from(31),
        U256::from(32),
        U256::MAX - U256::from(64),
    ] {
        let (outcome, host, _) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[value])),
            1_048_576,
        );
        if value <= U256::from(32) {
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(DS_MATH))
            );
        } else {
            assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
        }
    }
}
