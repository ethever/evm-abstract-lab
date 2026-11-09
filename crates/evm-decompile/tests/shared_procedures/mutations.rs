//! Supplied certificates are checked directly, without candidate rediscovery.
use super::{checked, fixtures};
use evm_decompile::{EntityId, SharedBinding, SharedInvocation, Stage, U256, ValueKind};

#[derive(Clone, Copy, Debug)]
enum Mutation {
    MissingBody,
    RecursiveBody,
    MissingDefinition,
    DuplicateDefinition,
    MissingProcedure,
    DuplicateInvocation,
    DuplicateInvocationIdentity,
    WrongOccurrence,
    WrongOwner,
    WrongPosition,
    WrongContinuation,
    MissingBinding,
    DuplicateFormal,
    DuplicateActual,
    InputPermutation,
    OutputPermutation,
    MissingInput,
    MissingOutput,
    MissingMemoryInput,
    MissingEffectInput,
    AliasedState,
    DropStore,
    DuplicateStore,
    ForbiddenCall,
    DropLog,
    DifferentOpcode,
    DifferentConstant,
    DifferentValueType,
    HiddenPathAssumption,
    EffectOrder,
    StaleInvocationValue,
    ArithmeticContract,
    LegacyContract,
}

fn call(stage: &Stage) -> SharedInvocation {
    stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .invocations
        .iter()
        .find(|call| {
            call.bindings.iter().any(|binding| {
                binding.formal != binding.actual && stage.ir.block(binding.formal).is_some()
            })
        })
        .expect("test requires an alpha-renamed source context, not just metadata")
        .clone()
}

fn operation(stage: &Stage, opcode: u8) -> (EntityId, EntityId) {
    let high = stage.ir.lifted.as_ref().unwrap();
    for call in &high.invocations {
        for binding in &call.bindings {
            if binding.formal != binding.actual
                && stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(|op| op.id == binding.formal && op.opcode == opcode)
            {
                return (binding.formal, binding.actual);
            }
        }
    }
    if let Some(op) = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == opcode)
    {
        return (op.id, op.id);
    }
    panic!("opcode 0x{opcode:02x} must be covered by actual alpha-context sharing")
}

fn mutate(stage: &mut Stage, mutation: Mutation) {
    let selected = call(stage);
    let high = stage.ir.lifted.as_mut().unwrap();
    let procedure_position = high
        .procedures
        .iter()
        .position(|procedure| procedure.id == selected.procedure)
        .unwrap();
    let call_position = high
        .invocations
        .iter()
        .position(|call| call.id == selected.id)
        .unwrap();
    match mutation {
        Mutation::ArithmeticContract => {
            high.procedures[procedure_position].kind =
                evm_decompile::SharedProcedureKind::PureArithmetic
        }
        Mutation::LegacyContract => {
            high.procedures[procedure_position].kind = evm_decompile::SharedProcedureKind::Legacy
        }
        Mutation::MissingBody => high.procedures[procedure_position].body = EntityId(u64::MAX),
        Mutation::RecursiveBody => high.procedures[procedure_position].body = selected.procedure,
        Mutation::MissingDefinition => {
            high.procedures.remove(procedure_position);
        }
        Mutation::DuplicateDefinition => high
            .procedures
            .push(high.procedures[procedure_position].clone()),
        Mutation::MissingProcedure => {
            high.invocations[call_position].procedure = EntityId(u64::MAX)
        }
        Mutation::DuplicateInvocation => high.invocations.push(selected),
        Mutation::DuplicateInvocationIdentity => {
            let other = high
                .invocations
                .iter_mut()
                .find(|call| call.id != selected.id)
                .expect("distinct valid sites");
            other.id = selected.id;
        }
        Mutation::WrongOccurrence => high.invocations[call_position].source = EntityId(u64::MAX),
        Mutation::WrongOwner => high.invocations[call_position].owner = Some(selected.source),
        Mutation::WrongPosition => high.invocations[call_position].position = usize::MAX,
        Mutation::WrongContinuation => {
            high.invocations[call_position].continuation = Some(selected.source)
        }
        Mutation::MissingBinding => {
            high.invocations[call_position].bindings.pop().unwrap();
        }
        Mutation::DuplicateFormal => {
            let binding = high.invocations[call_position].bindings[0];
            high.invocations[call_position].bindings.push(binding);
        }
        Mutation::DuplicateActual => {
            let bindings = &mut high.invocations[call_position].bindings;
            bindings[1].actual = bindings[0].actual;
        }
        Mutation::InputPermutation
        | Mutation::MissingInput
        | Mutation::OutputPermutation
        | Mutation::MissingOutput
        | Mutation::MissingMemoryInput
        | Mutation::MissingEffectInput
        | Mutation::AliasedState => match mutation {
            Mutation::InputPermutation => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| interface.inputs.len() >= 2)
                    .expect("two independent scalar inputs for permutation mutation");
                interface.inputs.swap(0, 1);
            }
            Mutation::MissingInput => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| !interface.inputs.is_empty())
                    .expect("explicit scalar inputs");
                interface.inputs.pop().expect("explicit scalar inputs");
            }
            Mutation::OutputPermutation => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| interface.outputs.len() >= 2)
                    .expect("simultaneous live scalar outputs");
                interface.outputs.swap(0, 1);
            }
            Mutation::MissingOutput => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| !interface.outputs.is_empty())
                    .expect("live scalar outputs");
                interface.outputs.pop().unwrap();
            }
            Mutation::MissingMemoryInput => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| !interface.memory_inputs.is_empty())
                    .expect("explicit memory dependency");
                interface
                    .memory_inputs
                    .pop()
                    .expect("explicit memory dependency");
            }
            Mutation::MissingEffectInput => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| !interface.effect_inputs.is_empty())
                    .expect("explicit effect dependency");
                interface
                    .effect_inputs
                    .pop()
                    .expect("explicit effect dependency");
            }
            Mutation::AliasedState => {
                let interface = high
                    .procedures
                    .iter_mut()
                    .map(|procedure| &mut procedure.interface)
                    .find(|interface| {
                        !interface.memory_inputs.is_empty() && !interface.effect_inputs.is_empty()
                    })
                    .expect("separate memory and effect dependencies");
                interface.memory_inputs = interface.effect_inputs.clone();
            }
            _ => unreachable!(),
        },
        Mutation::DropStore
        | Mutation::DuplicateStore
        | Mutation::ForbiddenCall
        | Mutation::DropLog
        | Mutation::DifferentOpcode
        | Mutation::HiddenPathAssumption
        | Mutation::EffectOrder => {
            let opcode = match mutation {
                Mutation::DropLog => 0xa0,
                _ => 0x55,
            };
            let (_, actual) = operation(stage, opcode);
            let block = stage
                .ir
                .blocks
                .iter_mut()
                .find(|block| block.operations.iter().any(|op| op.id == actual))
                .unwrap();
            let position = block
                .operations
                .iter()
                .position(|op| op.id == actual)
                .unwrap();
            match mutation {
                Mutation::DropStore | Mutation::DropLog => {
                    block.operations.remove(position);
                }
                Mutation::DuplicateStore => {
                    block
                        .operations
                        .insert(position, block.operations[position].clone());
                }
                Mutation::DifferentOpcode => block.operations[position].opcode = 0x5d,
                Mutation::ForbiddenCall => block.operations[position].opcode = 0xfa,
                Mutation::HiddenPathAssumption => {
                    block.operations[position].replacement = Some(U256::ZERO)
                }
                Mutation::EffectOrder => {
                    block.operations[position].effect_input =
                        block.operations[position].effect_output;
                }
                _ => unreachable!(),
            }
        }
        Mutation::DifferentConstant => {
            let expression = high
                .invocations
                .iter()
                .filter(|call| {
                    high.procedures
                        .iter()
                        .find(|procedure| procedure.id == call.procedure)
                        .is_some_and(|procedure| high.expression(procedure.body).is_none())
                })
                .flat_map(|call| &call.bindings)
                .find_map(|binding| {
                    if binding.formal == binding.actual {
                        return None;
                    }
                    high.expression(binding.actual)
                        .filter(|expression| {
                            matches!(
                                expression.kind,
                                evm_decompile::LiftedExpressionKind::Word(_)
                            )
                        })
                        .map(|expression| expression.id)
                })
                .expect("normalized literal inside an alpha-shared body");
            let expression = high
                .expressions
                .iter_mut()
                .find(|candidate| candidate.id == expression)
                .unwrap();
            let evm_decompile::LiftedExpressionKind::Word(word) = expression.kind else {
                unreachable!()
            };
            expression.kind =
                evm_decompile::LiftedExpressionKind::Word(word.wrapping_add(U256::from(1)));
        }
        Mutation::DifferentValueType => {
            let (_, actual) = operation(stage, 0x54);
            let output = stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .find(|op| op.id == actual)
                .unwrap()
                .outputs[0];
            stage
                .ir
                .values
                .iter_mut()
                .find(|value| value.id == output)
                .unwrap()
                .kind = ValueKind::Memory {
                operation: Some(actual),
            };
        }
        Mutation::StaleInvocationValue => {
            let old = high
                .invocations
                .iter()
                .find(|call| call.procedure == selected.procedure && call.id != selected.id)
                .expect("two independent invocations of this definition");
            let interface = &high.procedures[procedure_position].interface;
            let formal = interface
                .inputs
                .iter()
                .chain(&interface.outputs)
                .copied()
                .find(|formal| old.actual(*formal) != selected.actual(*formal))
                .expect("distinct dynamic scalar captures");
            let wrong = old.actual(formal).unwrap();
            let bindings = &mut high.invocations[call_position].bindings;
            if let Some(binding) = bindings.iter_mut().find(|binding| binding.formal == formal) {
                binding.actual = wrong;
            } else {
                bindings.push(SharedBinding {
                    formal,
                    actual: wrong,
                });
                bindings.sort_by_key(|binding| binding.formal);
            }
        }
    }
}

fn rejects(original: &Stage, mutation: Mutation) {
    let mut stage = original.clone();
    mutate(&mut stage, mutation);
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        high.validate_shared_sources(&stage.ir, |_| true).is_err(),
        "admitted {mutation:?}"
    );
    if !matches!(
        mutation,
        Mutation::DropStore
            | Mutation::DuplicateStore
            | Mutation::ForbiddenCall
            | Mutation::DropLog
            | Mutation::DifferentOpcode
            | Mutation::DifferentValueType
            | Mutation::HiddenPathAssumption
            | Mutation::EffectOrder
    ) {
        assert!(
            high.validate_shared_sources(&original.ir, |_| true)
                .is_err(),
            "detached supplied certificate {mutation:?} was checked against the original embedded program instead"
        );
    }
    let index = high.render_index(&stage.ir);
    assert_eq!(
        index.procedures().count(),
        0,
        "corrupt plan must preserve full safe display: {mutation:?}"
    );
    for call in &high.invocations {
        assert!(
            index.shared_invocation(call.source).is_none(),
            "unsafe omission for {mutation:?}"
        );
    }
}

#[test]
fn direct_source_admission_rejects_corrupt_definitions_calls_maps_and_effects() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    let original = artifact.current().unwrap();
    original
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .validate_shared_sources(&original.ir, |_| true)
        .unwrap();
    let pure_fixture = fixtures::pure_call_inputs();
    let pure_artifact = checked(&pure_fixture.input);
    // Automatic scope joining attempts every case and fails the test on any panic.
    std::thread::scope(|scope| {
        for mutation in [
            Mutation::MissingBody,
            Mutation::RecursiveBody,
            Mutation::MissingDefinition,
            Mutation::DuplicateDefinition,
            Mutation::MissingProcedure,
            Mutation::DuplicateInvocation,
            Mutation::DuplicateInvocationIdentity,
            Mutation::WrongOccurrence,
            Mutation::WrongOwner,
            Mutation::WrongPosition,
            Mutation::WrongContinuation,
            Mutation::MissingBinding,
            Mutation::DuplicateFormal,
            Mutation::DuplicateActual,
            Mutation::OutputPermutation,
            Mutation::MissingInput,
            Mutation::MissingOutput,
            Mutation::MissingMemoryInput,
            Mutation::MissingEffectInput,
            Mutation::AliasedState,
            Mutation::DropStore,
            Mutation::DuplicateStore,
            Mutation::ForbiddenCall,
            Mutation::DropLog,
            Mutation::DifferentOpcode,
            Mutation::DifferentConstant,
            Mutation::DifferentValueType,
            Mutation::HiddenPathAssumption,
            Mutation::EffectOrder,
            Mutation::StaleInvocationValue,
            Mutation::ArithmeticContract,
            Mutation::LegacyContract,
        ] {
            let source = match mutation {
                Mutation::DropLog => pure_artifact.current().unwrap(),
                _ => original,
            };
            scope.spawn(move || {
                rejects(source, mutation);
                eprintln!("direct supplied source mutation rejected: {mutation:?}");
            });
        }
    });
}

#[test]
fn direct_source_admission_rejects_two_live_input_permutation() {
    let fixture = fixtures::two_arguments();
    eprintln!(
        "shared two_arguments runtime: {}",
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode)
    );
    let artifact = checked(&fixture.input);
    let original = artifact.current().unwrap();
    let host = evm_decompile::MemoryHost::new(
        &fixture.input,
        fixtures::words(&[1, 2, 3, 4].map(U256::from)),
    );
    let report = super::oracle::certify(
        &fixture.input,
        &artifact,
        host,
        &evm_decompile::ExecutionLimits::default(),
    );
    assert!(
        report.calls.len() >= 2,
        "both two-argument calls must execute normalized definitions"
    );
    assert_eq!(report.host.state.storage[&U256::from(1)], U256::from(3));
    assert_eq!(report.host.state.storage[&U256::from(2)], U256::from(7));
    let mut changed = original.clone();
    mutate(&mut changed, Mutation::InputPermutation);
    let high = changed.ir.lifted.as_ref().unwrap();
    assert!(
        high.validate_shared_sources(&original.ir, |_| true)
            .is_err()
    );
    assert!(high.validate_shared_sources(&changed.ir, |_| true).is_err());
}

#[test]
fn direct_admission_refuses_before_unpaid_index_and_correspondence_work() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.invocations.is_empty(),
        "resource refusal test requires real certificate work"
    );
    let mut requests = Vec::new();
    assert!(
        high.validate_shared_sources(&stage.ir, |units| {
            requests.push(units);
            false
        })
        .is_err()
    );
    assert_eq!(
        requests.len(),
        1,
        "first resource refusal must stop all further work"
    );
    assert!(requests[0] > 0);
}

#[test]
fn caller_metered_render_admission_stops_on_refusal_and_preserves_source() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(!high.procedures.is_empty());
    let mut requests = Vec::new();
    let index = high.render_index_with_budget(&stage.ir, |units| {
        requests.push(units);
        false
    });
    assert!(index.validate_shared_sources().is_err());
    assert_eq!(index.procedures().count(), 0);
    assert_eq!(
        requests.len(),
        1,
        "render preparation continued after caller refusal"
    );
    for call in &high.invocations {
        assert!(index.shared_invocation(call.source).is_none());
        assert!(
            super::boundaries::nodes(&stage.ir.structured.as_ref().unwrap().root)
                .iter()
                .any(|node| node.id == call.source)
                || high.expression(call.source).is_some(),
            "safe original source must remain reachable in its retained arena"
        );
    }
    let cancel = evm_decompile::Cancellation::new();
    let before = artifact.clone();
    let exported =
        evm_decompile::export_text_with_observer(&artifact, &cancel, u64::MAX, |_| cancel.cancel());
    assert_eq!(exported.status, evm_decompile::TextExportStatus::Cancelled);
    assert_eq!(
        artifact, before,
        "cancelled admission must not publish a partial plan"
    );
}

#[test]
fn directly_supplied_terminal_body_and_exit_splits_are_rejected() {
    use evm_decompile::{SharedProcedure, SharedSourceIndex, StructuredKind};
    let fixture = fixtures::repeated_tail(0xf3);
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let index = SharedSourceIndex::new(&stage.ir, &mut |_| true).unwrap();
    let all = super::boundaries::nodes(&stage.ir.structured.as_ref().unwrap().root);
    for exit_only in [false, true] {
        let occurrences: Vec<_> = all
            .iter()
            .copied()
            .filter(|node| {
                let block = match node.kind {
                    StructuredKind::Exit { block } if exit_only => block,
                    StructuredKind::Block { block } if !exit_only => block,
                    _ => return false,
                };
                stage
                    .ir
                    .block(block)
                    .unwrap()
                    .source
                    .is_some_and(|source| source.pc == fixture.labels["terminal"])
            })
            .collect();
        assert_eq!(
            occurrences.len(),
            2,
            "two distinct AST occurrences of the terminal source"
        );
        assert!(matches!(
            index.region(occurrences[0].id, &mut |_| true),
            Err(evm_decompile::SharedProcedureError::Region(_))
        ));
        assert!(matches!(
            index.region(occurrences[1].id, &mut |_| true),
            Err(evm_decompile::SharedProcedureError::Region(_))
        ));
        let mut supplied = stage.ir.lifted.as_ref().unwrap().clone();
        let procedure = EntityId(u64::MAX - 2);
        supplied.procedures = vec![SharedProcedure {
            id: procedure,
            kind: evm_decompile::SharedProcedureKind::Normal,
            body: occurrences[0].id,
            interface: evm_decompile::SharedInterface::default(),
        }];
        supplied.invocations = vec![
            index
                .invocation(EntityId(u64::MAX - 1), procedure, occurrences[0].id, vec![])
                .unwrap(),
            index
                .invocation(EntityId(u64::MAX), procedure, occurrences[1].id, vec![])
                .unwrap(),
        ];
        supplied.validate().unwrap();
        assert!(
            supplied
                .validate_shared_sources(&stage.ir, |_| true)
                .is_err(),
            "admitted terminal split exit_only={exit_only}"
        );
        let rendered = supplied.render_index(&stage.ir);
        assert_eq!(rendered.procedures().count(), 0);
        for call in &supplied.invocations {
            assert!(rendered.shared_invocation(call.source).is_none());
        }
    }
}

#[test]
fn detached_capture_cannot_read_a_value_defined_only_inside_its_invocation() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let mut supplied = stage.ir.lifted.as_ref().unwrap().clone();
    let (call_id, formal, actual, future) = supplied
        .invocations
        .iter()
        .find_map(|call| {
            let procedure = supplied
                .procedures
                .iter()
                .find(|procedure| procedure.id == call.procedure)
                .unwrap();
            let formal = *procedure.interface.inputs.first()?;
            let actual = call.actual(formal)?;
            if actual == formal {
                return None;
            }
            let future = procedure
                .interface
                .outputs
                .iter()
                .filter_map(|output| call.actual(*output))
                .find(|output| *output != actual)?;
            Some((call.id, formal, actual, future))
        })
        .expect("real alpha invocation with distinct captures and live outputs");
    let mut changed = 0;
    for expression in &mut supplied.expressions {
        if expression.kind == evm_decompile::LiftedExpressionKind::Value(actual) {
            expression.kind = evm_decompile::LiftedExpressionKind::Value(future);
            changed += 1;
        }
    }
    assert!(
        changed > 0,
        "actual dynamic input must be read by normalized expressions"
    );
    let call = supplied
        .invocations
        .iter_mut()
        .find(|call| call.id == call_id)
        .unwrap();
    if let Some(binding) = call
        .bindings
        .iter_mut()
        .find(|binding| binding.formal == formal)
    {
        binding.actual = future;
    } else {
        call.bindings.push(SharedBinding {
            formal,
            actual: future,
        });
        call.bindings.sort_by_key(|binding| binding.formal);
    }
    supplied.validate().unwrap();
    assert!(
        supplied
            .validate_shared_sources(&stage.ir, |_| true)
            .is_err(),
        "detached forward/stale capture must reject"
    );
    let mut in_place = stage.ir.clone();
    in_place.lifted = Some(supplied.clone());
    assert!(
        supplied
            .validate_shared_sources(&in_place, |_| true)
            .is_err(),
        "in-place forward/stale capture must reject before omission"
    );
    assert_eq!(supplied.render_index(&stage.ir).procedures().count(), 0);
}
