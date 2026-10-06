//! Environment regressions cover values, aliases, call rules and table boundaries.

use alloy_primitives::{Address, B256, U256};
use evm_abstract::{
    Fork,
    analysis::{self, Config, ExecutionConfig, FrontierReason, Status},
    bytecode::Program,
    domain::{Profile, Value},
    world::{Account, AddressInput, BlobHashes, ByteArray, Entry, EvmEnvironment, GasInput, World},
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn word(number: u64) -> Value {
    Value::constant(U256::from(number))
}
fn analyze(code: &str, environment: EvmEnvironment, profile: Profile) -> analysis::Analysis {
    analysis::analyze_with_environment(
        Program::from_hex(code).unwrap(),
        Config {
            domain_profile: profile,
            ..Config::default()
        },
        environment,
    )
    .unwrap()
}

#[test]
fn single_analysis_json_retains_symbolic_defaults_and_explicit_environment_assumptions() {
    let default = analyze("00", EvmEnvironment::default(), Profile::Product);
    let json = serde_json::to_value(&default).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(
        json["environment"],
        serde_json::to_value(default.environment()).unwrap()
    );
    assert_eq!(json["environment"]["to"]["Symbolic"], "To");
    assert_eq!(json["environment"]["caller"]["Symbolic"], "Caller");
    assert!(json["environment"]["origin"].is_null());
    assert_eq!(json["environment"]["value"], "Top");
    assert_eq!(json["environment"]["calldata"]["length"], "Top");
    assert!(json["environment"].get("input_scope").is_none());
    assert!(json.get("execution").is_none());

    let environment = EvmEnvironment {
        to: address(0x101).into(),
        caller: address(0x102).into(),
        origin: Some(address(0x103).into()),
        value: word(7),
        calldata: ByteArray::exact(&[0x2a]),
        number: word(42),
        gas_price: word(9),
        gas: GasInput::UpperBound(U256::from(30_000)),
        block_hashes: [(U256::from(41), B256::repeat_byte(0x11))].into(),
        blob_hashes: BlobHashes::exact(&[B256::repeat_byte(0x22)]),
        ..EvmEnvironment::default()
    };
    let observed = serde_json::to_value(&environment).unwrap();
    let result = analyze("00", environment, Profile::Product);
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["environment"], observed);
    assert_eq!(
        json["environment"]["to"]["Concrete"],
        serde_json::to_value(address(0x101)).unwrap()
    );
    assert_eq!(json["environment"]["value"]["Constants"][0], "0x7");
    assert_eq!(json["environment"]["number"]["Constants"][0], "0x2a");
    assert_eq!(
        json["environment"]["calldata"]["length"]["Constants"][0],
        "0x1"
    );
}

#[test]
fn omitted_inputs_are_symbolic_and_origin_aliases_caller_across_blocks() {
    // CALLER survives a jump before ORIGIN is read; EQ must remain exact.
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze("336004565b321400", EvmEnvironment::default(), profile);
        assert_eq!(result.status(), Status::Converged);
        assert_eq!(
            result.states().last().unwrap().exit_stack[0].singleton(),
            Some(U256::from(1))
        );
        let environment = result.environment();
        assert_eq!(environment.resolved_origin(), environment.caller);
        assert!(environment.to.as_concrete().is_none());
        assert!(environment.value.contains(U256::MAX));
        assert!(environment.calldata.len().constants().is_none());
        assert!(
            environment
                .calldata
                .byte_at(0, Default::default())
                .contains(U256::from(255))
        );
    }
}

#[test]
fn independent_environments_have_distinct_input_namespaces_and_clones_preserve_inputs() {
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let environment = EvmEnvironment::default();
        let first = analyze("333200", environment.clone(), profile);
        let cloned = analyze("333200", environment.clone(), profile);
        let independent = analyze("333200", EvmEnvironment::default(), profile);
        let left = &first.states()[0].exit_stack;
        let same = &cloned.states()[0].exit_stack;
        let other = &independent.states()[0].exit_stack;
        assert!(left[0].provenance().same_identity(left[1].provenance()));
        assert!(left[0].provenance().same_identity(same[0].provenance()));
        assert!(!left[0].provenance().same_identity(other[0].provenance()));
        let domain = if profile == Profile::Product {
            evm_abstract::domain::Domain::default()
        } else {
            evm_abstract::domain::Domain::new(std::num::NonZeroUsize::new(8).unwrap())
        };
        let comparison = domain.apply(
            revm_bytecode::opcode::EQ,
            &[left[0].clone(), other[0].clone()],
        );
        assert!(comparison.contains(U256::ZERO) && comparison.contains(U256::from(1)));
        assert_eq!(
            serde_json::to_value(first.environment()).unwrap(),
            serde_json::to_value(independent.environment()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&left[0]).unwrap(),
            serde_json::to_value(&other[0]).unwrap()
        );
    }
}

#[test]
fn caller_origin_alias_is_retained_at_control_flow_joins_in_each_profile() {
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analysis::analyze_with_environment(
            Program::from_hex("3334600857600c565b600c565b321400").unwrap(),
            Config {
                domain_profile: profile,
                context_depth: 0,
                ..Config::default()
            },
            EvmEnvironment::default(),
        )
        .unwrap();
        let merge = result
            .states()
            .iter()
            .find(|state| result.program().blocks()[state.key.basic_block_index].start_pc == 12)
            .unwrap();
        assert_eq!(merge.exit_stack[0].singleton(), Some(U256::from(1)));
    }
}

#[test]
fn independently_overridden_origin_is_never_assumed_equal_to_caller() {
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(
            "336004565b321400",
            EvmEnvironment {
                origin: Some(AddressInput::unknown_origin()),
                ..EvmEnvironment::default()
            },
            profile,
        );
        let comparison = &result.states().last().unwrap().exit_stack[0];
        assert!(comparison.contains(U256::ZERO));
        assert!(comparison.contains(U256::from(1)));
        assert_ne!(
            result.environment().resolved_origin(),
            result.environment().caller
        );
    }
}

#[test]
fn immutable_words_and_calldata_reads_keep_identity_across_blocks() {
    for code in [
        "346004565b341400",
        "426004565b421400",
        "5f356005565b5f351400",
        "366004565b361400",
    ] {
        let result = analyze(code, EvmEnvironment::default(), Profile::Product);
        assert_eq!(
            result.status(),
            Status::Converged,
            "{code}: {:?}",
            result.frontiers()
        );
        assert_eq!(
            result.states().last().unwrap().exit_stack[0].singleton(),
            Some(U256::from(1)),
            "{code}"
        );
    }
}

#[test]
fn every_transaction_and_block_opcode_uses_its_supplied_input() {
    let environment = EvmEnvironment {
        to: address(0x101).into(),
        caller: address(0x102).into(),
        origin: Some(address(0x103).into()),
        value: word(4),
        calldata: ByteArray::exact(&[0xaa, 0xbb]),
        gas_price: word(5),
        coinbase: address(0x104).into(),
        timestamp: word(6),
        number: word(7),
        prevrandao: word(8),
        gas_limit: word(9),
        chain_id: Some(word(10)),
        base_fee: word(11),
        blob_base_fee: word(12),
        gas: GasInput::UpperBound(U256::from(30_000)),
        ..EvmEnvironment::default()
    };
    let result = analyze(
        "30323334363a414243444546484a5a00",
        environment,
        Profile::Product,
    );
    let values = &result.states()[0].exit_stack;
    let expected = [0x101, 0x103, 0x102, 4, 2, 5, 0x104, 6, 7, 8, 9, 10, 11, 12];
    for (value, expected) in values.iter().zip(expected) {
        assert_eq!(value.singleton(), Some(U256::from(expected)));
    }
    let gas = values.last().unwrap();
    assert!(gas.contains(U256::ZERO));
    assert!(gas.contains(U256::from(30_000)));
    assert!(!gas.contains(U256::from(30_001)));
    assert!(gas.singleton().is_none());
    assert!(
        !result
            .diagnostics()
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, analysis::DiagnosticKind::OpaqueResult))
    );
}

#[test]
fn blockhash_checks_the_full_width_window_and_preserves_unknown_valid_holes() {
    let mut environment = EvmEnvironment {
        number: word(300),
        ..EvmEnvironment::default()
    };
    environment
        .block_hashes
        .insert(U256::from(44), B256::repeat_byte(0x44));
    environment
        .block_hashes
        .insert(U256::from(299), B256::repeat_byte(0x29));
    // lower boundary, too old, current, future, valid missing observation, MAX.
    let code = format!(
        "602c40602b4061012c4061012d4061012a407f{}4000",
        "ff".repeat(32)
    );
    let result = analyze(&code, environment, Profile::Product);
    let values = &result.states()[0].exit_stack;
    assert_eq!(
        values[0].singleton(),
        Some(U256::from_be_slice(B256::repeat_byte(0x44).as_slice()))
    );
    for index in [1, 2, 3, 5] {
        assert_eq!(values[index].singleton(), Some(U256::ZERO));
    }
    assert!(values[4].contains(U256::ZERO) && values[4].contains(U256::MAX));
}

#[test]
fn blobhash_distinguishes_open_inputs_closed_lists_and_partial_holes() {
    let hash = B256::repeat_byte(0x11);
    for (blobs, expected_zero) in [
        (BlobHashes::default(), false),
        (BlobHashes::exact(&[]), true),
        (BlobHashes::exact(&[hash]), true),
        (
            BlobHashes {
                length: word(2),
                hashes: [(U256::ZERO, hash)].into(),
            },
            false,
        ),
    ] {
        let result = analyze(
            "5f4960014900",
            EvmEnvironment {
                blob_hashes: blobs,
                ..EvmEnvironment::default()
            },
            Profile::Product,
        );
        let second = &result.states()[0].exit_stack[1];
        assert_eq!(second.singleton() == Some(U256::ZERO), expected_zero);
        if !expected_zero {
            assert!(second.contains(U256::ZERO) && second.contains(U256::MAX));
        }
    }
    let result = analyze(
        "60074900",
        EvmEnvironment {
            blob_hashes: BlobHashes {
                length: Value::top(),
                hashes: [(U256::from(7), hash)].into(),
            },
            ..EvmEnvironment::default()
        },
        Profile::Product,
    );
    assert_eq!(
        result.states()[0].exit_stack[0].singleton(),
        Some(U256::from_be_slice(hash.as_slice()))
    );
}

fn call(op: u8, target: u16) -> String {
    let value = if matches!(op, 0xf1 | 0xf2) {
        "6009"
    } else {
        ""
    };
    format!("5f5f5f5f{value}61{target:04x}6207a120{op:02x}")
}

#[test]
fn child_callers_addresses_values_and_origin_follow_each_call_kind() {
    for op in [0xf1, 0xf2, 0xf4, 0xfa] {
        let mut world = World::new(Fork::Osaka, "symbolic caller rules");
        let mut root = Account::from_hex(&format!("{}00", call(op, 0x200)), Fork::Osaka).unwrap();
        root.balance = word(1_000_000);
        world.insert(address(0x101), root).unwrap();
        world
            .insert(
                address(0x200),
                Account::from_hex("3032333400", Fork::Osaka).unwrap(),
            )
            .unwrap();
        let result = analysis::analyze_world(
            world,
            Entry::new(address(0x101)),
            ExecutionConfig::default(),
        )
        .unwrap();
        assert_eq!(result.status(), Status::Converged);
        let child = result
            .states()
            .iter()
            .find(|state| state.key.frames.len() == 2)
            .unwrap();
        let frame = child.entry.active();
        assert_eq!(
            frame.key.address_value.as_concrete(),
            Some(address(if matches!(op, 0xf2 | 0xf4) {
                0x101
            } else {
                0x200
            }))
        );
        if op == 0xf4 {
            assert_eq!(frame.key.caller, AddressInput::unknown_caller());
            assert!(frame.call_value.singleton().is_none());
        } else {
            assert_eq!(frame.key.caller.as_concrete(), Some(address(0x101)));
            assert_eq!(
                frame.call_value.singleton(),
                Some(U256::from(if op == 0xfa { 0 } else { 9 }))
            );
        }
        assert!(
            child.exit_stack[1].singleton().is_none(),
            "ORIGIN remains symbolic in child"
        );
    }
}

#[test]
fn child_independent_callvalue_never_aliases_unknown_root_callvalue() {
    let mut world = World::new(Fork::Osaka, "independent unknown call values");
    // Child CALLVALUE comes from an unknown storage word, not root CALLVALUE.
    let mut root = Account::from_hex("5f5f5f5f5f546102006207a120f100", Fork::Osaka).unwrap();
    root.storage_unknown = true;
    root.balance = word(1_000_000);
    world.insert(address(0x101), root).unwrap();
    world
        .insert(
            address(0x200),
            Account::from_hex("3400", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let result = analysis::analyze_world(
        world,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let child = result
        .states()
        .iter()
        .find(|state| state.key.frames.len() == 2)
        .unwrap();
    assert!(
        !child
            .entry
            .active()
            .call_value
            .provenance()
            .same_identity(result.states()[0].entry.active().call_value.provenance())
    );
}

#[test]
fn callee_calldata_does_not_alias_root_input_when_summaries_detach_a_relative_root() {
    let call = "6020602060205f6102006207a120fa";
    let code = format!("5f355f545f52{call}50{call}506020511400");
    for summaries in [true, false] {
        let mut world = World::new(Fork::Osaka, "independent arrays in summaries");
        let mut root = Account::from_hex(&code, Fork::Osaka).unwrap();
        root.storage_unknown = true;
        world.insert(address(0x101), root).unwrap();
        world
            .insert(
                address(0x200),
                Account::from_hex("5f355f5260205ff3", Fork::Osaka).unwrap(),
            )
            .unwrap();
        let result = analysis::analyze_world(
            world,
            Entry::new(address(0x101)),
            ExecutionConfig {
                use_summaries: summaries,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(result.status(), Status::Converged);
        let comparison_pc = code.len() / 2 - 2;
        let compared = result
            .states()
            .iter()
            .find(|state| {
                state.active().code_address == address(0x101)
                    && state.executed_pcs.contains(&comparison_pc)
            })
            .expect("root EQ was executed");
        let comparison = &compared.exit_stack[0];
        assert!(comparison.contains(U256::ZERO));
        assert!(comparison.contains(U256::from(1)));
        for summary in result.summaries() {
            assert!(!summary.input.frame.state.environment_calldata);
        }
        if summaries {
            assert!(result.summary_stats().hits > 0);
        }
    }
}

#[test]
fn gas_reads_use_bounds_without_claiming_equal_remaining_gas() {
    let result = analyze(
        "5a6004565b5a1400",
        EvmEnvironment {
            gas: GasInput::UpperBound(U256::from(30_000)),
            ..EvmEnvironment::default()
        },
        Profile::Product,
    );
    let value = &result.states().last().unwrap().exit_stack[0];
    assert!(value.contains(U256::ZERO));
    assert!(value.contains(U256::from(1)));
}

#[test]
fn bytecode_without_a_concrete_creator_retains_a_creation_frontier() {
    let result = analyze("5f5f5ff000", EvmEnvironment::default(), Profile::Product);
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .execution()
            .frontiers()
            .iter()
            .any(|frontier| matches!(
                frontier.reason,
                FrontierReason::Creation(analysis::CreationBoundary::UnknownCreator)
            ))
    );
}

#[test]
fn symbolic_call_targets_and_unknown_calldata_copy_sizes_keep_typed_frontiers() {
    for (code, expected_target) in [("5f5f5f5f5f5f356207a120f100", true), ("365f5f3700", false)] {
        let result = analyze(code, EvmEnvironment::default(), Profile::Product);
        assert_eq!(result.status(), Status::Incomplete);
        assert!(
            result
                .execution()
                .frontiers()
                .iter()
                .any(|frontier| if expected_target {
                    matches!(frontier.reason, FrontierReason::UnknownTarget)
                } else {
                    matches!(frontier.reason, FrontierReason::Memory)
                })
        );
    }
}

#[test]
fn environment_is_part_of_summary_qualification_and_snapshot_identity_stays_fixed() {
    let mut world = World::anchored(
        Fork::Osaka,
        U256::from(1),
        B256::repeat_byte(0x11),
        "environment summary",
    );
    world
        .insert(
            address(0x101),
            Account::from_hex(
                &format!("{}50{}5000", call(0xfa, 0x200), call(0xfa, 0x200)),
                Fork::Osaka,
            )
            .unwrap(),
        )
        .unwrap();
    world
        .insert(
            address(0x200),
            Account::from_hex("3233433a4800", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let mut entry = Entry::new(address(0x101));
    entry.environment.chain_id = Some(word(56));
    let result = analysis::analyze_world(world, entry.clone(), ExecutionConfig::default()).unwrap();
    assert!(result.summary_stats().hits > 0);
    for summary in result.summaries() {
        assert_eq!(summary.input.environment, entry.environment);
        let mut changed = summary.input.clone();
        changed.environment.gas_price = word(9);
        assert_ne!(summary.input, changed);
    }
    assert!(
        matches!(result.world().identity(),evm_abstract::world::SnapshotIdentity::Chain{chain_id,..} if *chain_id==U256::from(1))
    );
}

#[test]
fn raw_internal_owner_does_not_supply_code_for_an_unrelated_literal_address() {
    let anchor = Address::repeat_byte(0x11);
    let literal = format!("73{anchor:x}");
    let code = format!("{literal}3b{literal}3f60205f5f{literal}3c5f51303b00");
    let result = analyze(&code, EvmEnvironment::default(), Profile::Product);
    assert_eq!(result.status(), Status::Converged);
    let values = &result.states()[0].exit_stack;
    for value in &values[..3] {
        assert!(
            value.constants().is_none(),
            "literal address cannot reuse hidden root code"
        );
        assert!(value.contains(U256::ZERO) && value.contains(U256::MAX));
    }
    assert_eq!(
        values[3].singleton(),
        Some(U256::from(result.program().byte_len()))
    );
    let call = format!("5f5f5f5f5f{literal}6207a120f100");
    let result = analyze(&call, EvmEnvironment::default(), Profile::Product);
    assert_eq!(result.status(), Status::Incomplete);
    assert!(result.execution().frontiers().iter().any(
        |frontier| matches!(frontier.reason,FrontierReason::MissingCode(target) if target==anchor)
    ));
}

#[test]
fn contradictory_destinations_blob_counts_and_unbounded_tables_fail_admission() {
    let mut environment = EvmEnvironment::default();
    environment.blob_hashes.length = word(0);
    environment
        .blob_hashes
        .hashes
        .insert(U256::ZERO, B256::ZERO);
    assert!(environment.validate().is_err());
    environment = EvmEnvironment::default();
    environment.block_hashes = (0..257)
        .map(|index| (U256::from(index), B256::ZERO))
        .collect();
    assert!(environment.validate().is_err());
    let mut world = World::new(Fork::Osaka, "destination admission");
    world.insert(address(0x101), Account::empty()).unwrap();
    let mut entry = Entry::new(address(0x101));
    entry.environment.to = address(0x102).into();
    assert!(analysis::analyze_world(world, entry, ExecutionConfig::default()).is_err());
}
