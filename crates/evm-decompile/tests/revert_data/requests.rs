//! Resource premises are local to the original requests and their fault prefix.
use super::*;
use evm_decompile::{
    ExecutionDomain, ExecutionLimits, LiftedRevertFormat, Primitive, SemanticHost,
    execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

const REQUEST: &str =
    include_str!("../fixtures/revert-data/custom-symbolic-request-range-guard.hex");

fn artifact(domain: ExecutionDomain) -> (Input, Artifact) {
    let input = input(REQUEST);
    let mut config = Config {
        execution_domain: domain,
        ..Config::default()
    };
    config.readability.procedures = ProcedurePreference::Inline;
    let artifact = decompile(&input, &config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    (input, artifact)
}

#[test]
fn opaque_request_ranges_need_the_declared_domain_and_keep_lazy_faults() {
    let arguments = [U256::from(17), U256::MAX];
    for domain in [
        ExecutionDomain::Generic,
        ExecutionDomain::EthereumTransactions,
    ] {
        let (input, artifact) = artifact(domain);
        let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
        assert_eq!(program.revert_data.len(), 1);
        let data = &program.revert_data[0];
        if domain.is_generic() {
            assert!(matches!(data.content, LiftedRevertDataKind::Raw));
            assert!(program.constructions.is_empty());
        } else {
            assert!(matches!(data.decode_format(|_| true).unwrap(),
                LiftedRevertFormat::SelectorWords { selector: [0xd4, 0xd8, 0xf3, 0xe6], ref words }
                    if words.len() == 2));
            assert_eq!(program.constructions.len(), 1);
            assert!(render_text(&artifact).contains("require("));
            assert!(program.controls.iter().any(|control| {
                control.guard.as_ref().is_some_and(
                    |guard| matches!(guard.payload, LiftedGuardPayload::Data(id) if id == data.id),
                )
            }));
        }
        for success in [false, true] {
            for pointer in [U256::ZERO, U256::from(64), U256::from(128), U256::MAX] {
                for limit in [0, 64, 96, 160, 224, 4096] {
                    let words = [pointer, arguments[0], arguments[1]];
                    let state = host(&input, success, &words);
                    let (outcome, state, capsules) =
                        payload_oracle::certify(&input, &artifact, state, limit);
                    if success {
                        assert_eq!(
                            capsules, 0,
                            "successful route executed a failure constructor"
                        );
                    }
                    let end = usize::try_from(pointer)
                        .ok()
                        .and_then(|p| p.checked_add(96));
                    if limit < 96 || (!success && end.is_none_or(|end| end > limit)) {
                        assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
                    } else if success {
                        assert!(matches!(
                            outcome.unwrap(),
                            ExecutionOutcome::Exit {
                                kind: ExitKind::Stop,
                                ..
                            }
                        ));
                    } else {
                        let expected = [
                            vec![0xd4, 0xd8, 0xf3, 0xe6],
                            arguments
                                .into_iter()
                                .flat_map(|word| word.to_be_bytes::<32>())
                                .collect(),
                        ]
                        .concat();
                        assert_eq!(
                            payload_oracle::reverted(&outcome.unwrap(), &state),
                            Some(expected)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn request_certificates_recheck_domain_fork_and_actual_source_operations() {
    let (_, artifact) = artifact(ExecutionDomain::EthereumTransactions);
    for corruption in 0..7 {
        let mut stage = artifact.current().unwrap().clone();
        let proof = stage.ir.lifted.as_ref().unwrap().revert_data[0].clone();
        match corruption {
            0 => {
                stage.ir.execution_domain = ExecutionDomain::Generic;
                stage.ir.execution_fork = None;
                // Matching serialized metadata is insufficient: replay must
                // independently lose the conditional memory range premise.
                for data in &mut stage.ir.lifted.as_mut().unwrap().revert_data {
                    data.execution_domain = ExecutionDomain::Generic;
                    data.execution_fork = None;
                }
            }
            1 => stage.ir.execution_fork = None,
            2 => stage.ir.lifted.as_mut().unwrap().revert_data[0].memory = proof.offset,
            3 => stage.ir.lifted.as_mut().unwrap().revert_data[0].offset = proof.size,
            4 => {
                let initial = stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|b| &b.operations)
                    .find(|o| o.opcode == 0x52)
                    .unwrap()
                    .id;
                stage.ir.lifted.as_mut().unwrap().revert_data[0].writes[0] = initial;
            }
            5 => {
                let op = stage
                    .ir
                    .blocks
                    .iter_mut()
                    .flat_map(|b| &mut b.operations)
                    .find(|o| o.id == proof.operation)
                    .unwrap();
                op.opcode = 0xf3;
            }
            6 => {
                let op = stage
                    .ir
                    .blocks
                    .iter_mut()
                    .flat_map(|b| &mut b.operations)
                    .find(|o| o.id == proof.operation)
                    .unwrap();
                op.effects.memory_read = false;
            }
            _ => unreachable!(),
        }
        assert_ne!(
            validate_stage(&stage).status,
            ValidationStatus::Passed,
            "accepted request corruption {corruption}"
        );
    }
}

#[test]
fn a_transaction_request_bound_does_not_establish_unknown_aliases_or_sibling_writes() {
    // The saved P is shared, but the second request has an independent offset.
    let mut a = Asm::default();
    a.number(0).op(0x35).op(0x80).number(0x42).op(0x90).op(0x52);
    a.number(0xff).number(32).op(0x35).op(0x52);
    a.number(32).op(0x90).op(0xfd);
    let input = a.finish();
    let config = Config {
        execution_domain: ExecutionDomain::EthereumTransactions,
        ..Config::default()
    };
    let artifact = decompile(&input, &config);
    let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(
        program
            .revert_data
            .iter()
            .all(|data| matches!(data.content, LiftedRevertDataKind::Raw))
    );
    assert!(program.constructions.is_empty());

    let mut a = Asm::default();
    a.number(0).op(0x35).target("written").op(0x57);
    a.target("exit").op(0x56);
    a.dest("written").number(0x42).number(0).op(0x52);
    a.dest("exit").number(32).number(0).op(0xfd);
    let artifact = decompile(&a.finish(), &config);
    let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(
        program
            .revert_data
            .iter()
            .all(|data| matches!(data.content, LiftedRevertDataKind::Raw))
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SparseHost {
    calldata: Vec<u8>,
    // The carry bit preserves mathematical byte indices above U256::MAX.
    memory: BTreeMap<(bool, U256), u8>,
    reverted: Vec<u8>,
}

impl SparseHost {
    fn key(start: U256, index: usize) -> (bool, U256) {
        let (offset, carry) = start.overflowing_add(U256::from(index));
        (carry, offset)
    }
}

impl SemanticHost for SparseHost {
    fn execute(&mut self, primitive: Primitive<'_>) -> Result<Vec<U256>, ExecutionError> {
        let args = primitive.inputs;
        match primitive.opcode {
            0x00 => Ok(Vec::new()),
            0x35 => {
                let start = usize::try_from(args[0]).unwrap();
                let mut bytes = [0; 32];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = self.calldata.get(start + index).copied().unwrap_or(0);
                }
                Ok(vec![U256::from_be_bytes(bytes)])
            }
            0x51 => {
                let mut bytes = [0; 32];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = self
                        .memory
                        .get(&Self::key(args[0], index))
                        .copied()
                        .unwrap_or(0);
                }
                Ok(vec![U256::from_be_bytes(bytes)])
            }
            0x52 => {
                for (index, byte) in args[1].to_be_bytes::<32>().into_iter().enumerate() {
                    self.memory.insert(Self::key(args[0], index), byte);
                }
                Ok(Vec::new())
            }
            0xfd => {
                self.reverted = (0..usize::try_from(args[1]).unwrap())
                    .map(|index| {
                        self.memory
                            .get(&Self::key(args[0], index))
                            .copied()
                            .unwrap_or(0)
                    })
                    .collect();
                Ok(Vec::new())
            }
            opcode => Err(ExecutionError::UnsupportedPrimitive(opcode)),
        }
    }
}

#[test]
fn generic_sparse_memory_can_accept_huge_requests_without_the_transaction_premise() {
    let (input, artifact) = artifact(ExecutionDomain::Generic);
    let pointer = U256::MAX - U256::from(9);
    let host = SparseHost {
        calldata: [U256::ZERO, pointer, U256::from(17), U256::from(19)]
            .into_iter()
            .flat_map(|word| word.to_be_bytes::<32>())
            .collect(),
        memory: BTreeMap::new(),
        reverted: Vec::new(),
    };
    let limits = ExecutionLimits::default();
    let mut raw = host.clone();
    let outcome = execute_bytecode(&input, &mut raw, &limits).unwrap().outcome;
    assert_eq!(
        &raw.reverted[..4],
        &[0, 0, 0, 0],
        "the selector was written above 2^256, not at the wrapped REVERT offset"
    );
    assert_eq!(raw.reverted.len(), 68);
    let stage = artifact.current().unwrap();
    let mut graph = host.clone();
    assert_eq!(
        execute_ir(stage, &mut graph, &limits).unwrap().outcome,
        outcome
    );
    assert_eq!(graph, raw);
    let mut syntax = host;
    assert_eq!(
        execute_structured(stage, &mut syntax, &limits)
            .unwrap()
            .outcome,
        outcome
    );
    assert_eq!(syntax, raw);
}
