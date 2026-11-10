//! External regressions exercise bytes, source admission and lazy fault behavior
//! independently of payload discovery and the producer's canonical rebuild.
use super::{assert_unchanged_witness, lifted, support::checked};
use evm_decompile::{
    Artifact, Config, ExecutionError, ExecutionOutcome, ExitKind, ExternalResponse,
    LiftedGuardPayload, MemoryHost, StageKind, U256, decompile, render_text,
};

#[path = "error_payloads/depth.rs"]
mod depth;
#[path = "error_payloads/domain_bounds.rs"]
mod domain_bounds;
#[path = "error_payloads/fixtures.rs"]
mod fixtures;
#[path = "error_payloads/oracle.rs"]
mod oracle;
#[path = "error_payloads/pair.rs"]
mod pair;
#[path = "error_payloads/proofs.rs"]
mod proofs;
#[path = "error_payloads/real_case.rs"]
mod real_case;
#[path = "error_payloads/resources.rs"]
mod resources;

use fixtures::{Asm, DS_MATH, Mutation, TRANSFER, begin, calldata, end, initialize};

fn error_guards(artifact: &Artifact) -> usize {
    lifted(artifact)
        .1
        .controls
        .iter()
        .filter(|control| {
            control
                .guard
                .as_ref()
                .is_some_and(|guard| matches!(guard.payload, LiftedGuardPayload::ErrorString(_)))
        })
        .count()
}

fn assert_message(artifact: &Artifact, message: &[u8]) {
    let (stage, program) = lifted(artifact);
    let payload = program
        .payloads
        .iter()
        .find(|payload| payload.message.as_bytes() == message)
        .unwrap_or_else(|| {
            panic!(
                "missing exact message {message:?}: {}",
                render_text(artifact)
            )
        });
    assert_eq!(payload.bytes, oracle::abi(message));
    let reverts: Vec<_> = program
        .reverts
        .iter()
        .filter(|proof| proof.payload == payload.id)
        .collect();
    assert!(
        !reverts.is_empty(),
        "payload has no terminal memory certificate"
    );
    for proof in reverts {
        let operation = stage
            .ir
            .block(proof.block)
            .unwrap()
            .operations
            .last()
            .unwrap();
        assert_eq!(operation.opcode, 0xfd);
        assert_eq!(operation.id, proof.operation);
        assert_eq!(operation.memory_input, Some(proof.memory));
        assert_eq!(operation.inputs, vec![proof.offset, proof.size]);
    }
    assert_unchanged_witness(artifact);
}

#[path = "error_payloads/guards.rs"]
mod guards;
#[path = "error_payloads/loops.rs"]
mod loops;
#[path = "error_payloads/memory.rs"]
mod memory;
#[path = "error_payloads/strings.rs"]
mod strings;
