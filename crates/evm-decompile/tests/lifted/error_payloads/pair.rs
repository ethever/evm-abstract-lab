//! The reported Pair failure path remains pinned to its genuine source PCs.
use super::*;
use evm_decompile::{Completion, EntityId, ExecutionDomain, Fork, Input};
use std::sync::OnceLock;

fn artifacts() -> &'static [Artifact; 2] {
    static ARTIFACTS: OnceLock<[Artifact; 2]> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let bytes = evm_bytecode::parse_hex(include_str!(
            "../../fixtures/structure/ethereum-uniswap-v2-usdc-weth-pair.hex"
        ))
        .unwrap();
        assert_eq!(bytes.len(), 11293);
        assert_eq!(
            format!("{:x}", alloy_primitives::keccak256(&bytes)),
            "5b83bdbcc56b2e630f2807bbadd2b0c21619108066b92a58de081261089e9ce5"
        );
        let input = Input::new(bytes, Fork::Osaka);
        [
            ExecutionDomain::Generic,
            ExecutionDomain::EthereumTransactions,
        ]
        .map(|execution_domain| {
            let artifact = decompile(
                &input,
                &Config {
                    execution_domain,
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
        })
    })
}

#[test]
fn genuine_pair_second_transfer_failure_has_a_source_checked_bounded_payload() {
    let [generic, ethereum] = artifacts();
    for artifact in [generic, ethereum] {
        let (stage, program) = lifted(artifact);
        let block = stage
            .ir
            .blocks
            .iter()
            .find(|block| block.operations.iter().any(|op| op.id == EntityId(17791)))
            .unwrap();
        let terminal = block.operations.last().unwrap();
        assert_eq!(terminal.opcode, 0xfd);
        assert_eq!(terminal.source.unwrap().pc, 8672);
        let proof = program
            .reverts
            .iter()
            .find(|proof| proof.operation == terminal.id);
        let text = render_text(artifact);
        let raw_first_load = "v17739 = evm.mload(offset = 0x40)";
        if artifact.config.execution_domain == ExecutionDomain::Generic {
            assert!(
                proof.is_none(),
                "Generic execution must retain the unproved source constructor"
            );
            assert!(
                text.contains(raw_first_load),
                "the reported Generic constructor stays visible"
            );
        } else {
            let proof = proof.expect("the original PC8571..8673 constructor must certify");
            assert_eq!(proof.execution_domain, stage.ir.execution_domain);
            assert_eq!(proof.execution_fork, stage.ir.execution_fork);
            let payload = program
                .payloads
                .iter()
                .find(|payload| payload.id == proof.payload)
                .unwrap();
            assert_eq!(payload.message, "UniswapV2: TRANSFER_FAILED");
            assert_eq!(payload.bytes, oracle::abi(payload.message.as_bytes()));
            let pcs: Vec<_> = proof
                .writes
                .iter()
                .map(|id| {
                    block
                        .operations
                        .iter()
                        .find(|op| op.id == *id)
                        .unwrap()
                        .source
                        .unwrap()
                        .pc
                })
                .collect();
            assert_eq!(pcs, [8609, 8616, 8623, 8661]);
            let index = program.render_index(&stage.ir);
            index.validate_guard_sources().unwrap();
            assert_eq!(
                index.render_revert(block.id, |_| true).unwrap(),
                Some("evm.revert(\"UniswapV2: TRANSFER_FAILED\");".into())
            );
            assert!(
                !text.contains(raw_first_load),
                "the selected bounded output must fold the actual reported constructor"
            );
            let predecessor = stage
                .ir
                .edges
                .iter()
                .find(|edge| edge.to == block.id)
                .unwrap()
                .from;
            let guard = program.controls.iter().find(|control| {
                control.guard.as_ref().is_some_and(|guard| {
                    guard.block == predecessor
                        && guard.payload == LiftedGuardPayload::ErrorString(proof.payload)
                })
            });
            if let Some(control) = guard {
                let statement = index.render_guard(control, |_| true).unwrap().unwrap();
                assert!(
                    text.contains(&statement),
                    "a qualified source If must appear as its exact require in selected output"
                );
                eprintln!("Pair PC8672 selected output: {statement}");
            } else {
                assert!(text.contains("evm.revert(\"UniswapV2: TRANSFER_FAILED\");"));
                eprintln!(
                    "Pair PC8672 source constructor folded; its owning If has no qualified require plan"
                );
            }
        }
        assert_unchanged_witness(artifact);
    }
}
