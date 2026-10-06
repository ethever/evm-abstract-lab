//! Readable world reports preserve distinct paths, full identities and sparse byte facts.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        ExecutionConfig, FrontierReason, OutcomeKind, Status, WorldAnalysis, analyze_world,
    },
    domain::Value,
    render,
    world::{Account, ByteArray, Entry, Existence, World},
};

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn entry() -> Entry {
    Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (Address::ZERO).into(),
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    }
}

fn number(text: &str) -> U256 {
    U256::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

/// Read the same checked-in fixture used by the user's analyze command.
fn fixture(name: &str) -> World {
    let path = format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut world = World::new(Fork::Osaka, json["provenance"].as_str().unwrap());
    for input in json["accounts"].as_array().unwrap() {
        let mut account = input["code"]
            .as_str()
            .map_or_else(Account::unknown, |code| {
                Account::from_hex(code, world.fork()).unwrap()
            });
        account.balance = input["balance"]
            .as_str()
            .map_or_else(Value::top, |value| Value::constant(number(value)));
        // Match the CLI's partial-observation boundary instead of inheriting
        // the synthetic constructor's known nonce and account presence.
        account.nonce = input["nonce"]
            .as_str()
            .map_or_else(Value::top, |value| Value::constant(number(value)));
        account.existence = match input["existence"].as_str() {
            None | Some("unknown") => Existence::Unknown,
            Some("present") => Existence::Present,
            Some("absent") => Existence::Absent,
            Some(value) => panic!("unsupported fixture existence {value}"),
        };
        account.storage_unknown = input["storage_unknown"].as_bool().unwrap_or(true);
        if let Some(storage) = input["storage"].as_object() {
            for (slot, value) in storage {
                account.storage.insert(
                    number(slot),
                    Value::constant(number(value.as_str().unwrap())),
                );
            }
        }
        world
            .insert(input["address"].as_str().unwrap().parse().unwrap(), account)
            .unwrap();
    }
    world
}

fn analyze(name: &str, config: ExecutionConfig) -> WorldAnalysis {
    analyze_world(fixture(name), entry(), config).unwrap()
}

fn heading(line: &str) -> &str {
    line.trim().trim_matches(['#', '[', ']']).trim()
}

fn section<'a>(text: &'a str, title: &str, next: &str) -> &'a str {
    let start = text
        .lines()
        .find(|line| heading(line) == title)
        .unwrap_or_else(|| panic!("missing section {title}:\n{text}"));
    let after = &text[text.find(start).unwrap() + start.len()..];
    if next.is_empty() {
        after
    } else {
        let end = after
            .lines()
            .find(|line| heading(line) == next)
            .unwrap_or_else(|| panic!("missing section {next}:\n{text}"));
        &after[..after.find(end).unwrap()]
    }
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

fn numbered<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    let offsets: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let first = cells(line)
                .first()?
                .trim()
                .trim_matches(['#', '[', ']'])
                .trim()
                .trim_end_matches(':');
            let suffix = first.strip_prefix(prefix)?;
            if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some(text.find(line).unwrap())
        })
        .collect();
    offsets
        .iter()
        .enumerate()
        .map(|(index, start)| &text[*start..offsets.get(index + 1).copied().unwrap_or(text.len())])
        .collect()
}

fn alias(text: &str, title: &str, next: &str, identity: &str) -> String {
    let legend = section(text, title, next);
    let row = legend
        .lines()
        .find(|line| cells(line).contains(&identity))
        .unwrap_or_else(|| panic!("missing full identity {identity}:\n{legend}"));
    cells(row)[0].to_owned()
}

#[test]
fn returndata_copy_report_has_sections_and_keeps_every_outcome_and_payload() {
    let analysis = analyze("returndata-copy", ExecutionConfig::default());
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return
                && outcome.data.exact_bytes() == Some([vec![0; 31], vec![1]].concat()))
    );
    let text = render::world::text(&analysis);
    for title in [
        "Analysis",
        "Snapshot",
        "Addresses",
        "Hashes",
        "States",
        "State details",
        "Transitions",
        "Outcomes",
        "Call summaries",
        "Diagnostics",
        "Frontiers",
    ] {
        assert!(
            text.lines().any(|line| heading(line) == title),
            "missing section {title}:\n{text}"
        );
    }
    let outcomes = numbered(section(&text, "Outcomes", "Call summaries"), "O");
    assert_eq!(
        outcomes.len(),
        analysis.outcomes().len(),
        "each possible completion needs its own numbered report"
    );
    for (index, (outcome, report)) in analysis.outcomes().iter().zip(outcomes).enumerate() {
        let header = cells(report.lines().next().unwrap());
        assert_eq!(header[0], format!("O{index}"));
        assert!(header.contains(&format!("S{}", outcome.state).as_str()));
        assert!(header.contains(&format!("{:?}", outcome.kind).as_str()));
        assert!(report.contains(&format!("length={}", outcome.data.len())));
        assert!(report.contains("default={0x0}"));
        if let Some(bytes) = outcome.data.exact_bytes()
            && !bytes.is_empty()
        {
            let hex = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert!(
                report.contains(&format!("exact hex: 0x{hex}")),
                "missing full returned bytes:\n{report}"
            );
            assert!(report.contains("0x001f") && report.contains("01"));
        }
        for ((owner, slot), value) in outcome.store.slots() {
            let owner = alias(&text, "Addresses", "Hashes", &owner.to_string());
            assert!(
                report.lines().any(|line| {
                    let row = cells(line);
                    row.contains(&owner.as_str())
                        && row.contains(&format!("{slot:#x}").as_str())
                        && row.contains(&value.to_string().as_str())
                }),
                "missing stored value in this outcome:\n{report}"
            );
        }
        let accounts = section(report, "Account observations", "Possible logs");
        let rows: Vec<_> = accounts
            .lines()
            .map(cells)
            .filter(|row| {
                row.first().is_some_and(|cell| {
                    cell.strip_prefix('A').is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                    })
                })
            })
            .collect();
        assert_eq!(
            rows.len(),
            outcome.store.addresses().len(),
            "all observed accounts belong to this outcome"
        );
        for address in outcome.store.addresses() {
            let reference = alias(&text, "Addresses", "Hashes", &address.to_string());
            let row = rows.iter().find(|row| row[0] == reference).unwrap();
            assert!(row.contains(&outcome.store.read_balance(address).to_string().as_str()));
            assert!(row.contains(&outcome.store.nonce(address).to_string().as_str()));
            assert!(row.contains(&format!("{:?}", outcome.store.existence(address)).as_str()));
        }
    }
}

#[test]
fn full_identity_legends_and_state_table_keep_proxy_code_and_storage_owners_distinct() {
    let analysis = analyze("proxy-storage", ExecutionConfig::default());
    let text = render::world::text(&analysis);
    let states = section(&text, "States", "State details");
    let rows: Vec<_> = states
        .lines()
        .map(cells)
        .filter(|row| {
            row.first().is_some_and(|cell| {
                cell.starts_with('S') && cell[1..].bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .collect();
    assert_eq!(rows.len(), analysis.states().len());
    for state in analysis.states() {
        let frame = state.active();
        let code = alias(
            &text,
            "Addresses",
            "Hashes",
            &frame.code_address.to_string(),
        );
        let owner = alias(&text, "Addresses", "Hashes", &frame.address.to_string());
        let caller = alias(&text, "Addresses", "Hashes", &frame.caller.to_string());
        let hash = alias(&text, "Hashes", "States", &frame.code_hash.to_string());
        let row = rows
            .iter()
            .find(|row| row[0] == format!("S{}", state.id))
            .unwrap();
        assert!(
            row.contains(&code.as_str())
                && row.contains(&owner.as_str())
                && row.contains(&caller.as_str())
                && row.contains(&hash.as_str())
        );
        assert!(row.contains(&format!("B{}", frame.basic_block_index).as_str()));
        assert!(row.contains(&format!("{:?}", frame.mode).as_str()));
        if frame.code_address == address(0x300) {
            assert_ne!(
                code, owner,
                "delegate execution must show implementation and storage owner separately"
            );
            assert!(
                matches!(frame.address, owner if owner == address(0x201) || owner == address(0x202))
            );
        }
    }
    assert!(
        text.contains(&analysis.world().fingerprint().to_string()),
        "snapshot fingerprint must remain complete"
    );
}

#[test]
fn incomplete_reports_preserve_all_frontiers_and_every_target_frame() {
    for (name, config) in [
        ("missing-code", ExecutionConfig::default()),
        (
            "call-return-branch",
            ExecutionConfig {
                max_work: 1,
                ..ExecutionConfig::default()
            },
        ),
        (
            "proxy-storage",
            ExecutionConfig {
                max_call_depth: 2,
                ..ExecutionConfig::default()
            },
        ),
    ] {
        let analysis = analyze(name, config);
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(!analysis.frontiers().is_empty());
        let text = render::world::text(&analysis);
        assert!(section(&text, "Analysis", "Snapshot").contains("Incomplete"));
        let reports = numbered(section(&text, "Frontiers", ""), "F");
        assert_eq!(
            reports.len(),
            analysis.frontiers().len(),
            "frontiers cannot be collapsed:\n{text}"
        );
        for (frontier, report) in analysis.frontiers().iter().zip(reports) {
            assert!(
                report.contains(&format!("{:?}", frontier.reason))
                    || matches!(frontier.reason, FrontierReason::MissingCode(_))
                        && report.contains("MissingCode")
            );
            if let Some(from) = frontier.from {
                assert!(report.contains(&format!("S{from}")));
            }
            if let Some(target) = &frontier.target {
                let frame_rows: Vec<_> = report
                    .lines()
                    .map(cells)
                    .filter(|row| {
                        row.first().is_some_and(|cell| {
                            !cell.is_empty() && cell.bytes().all(|byte| byte.is_ascii_digit())
                        })
                    })
                    .collect();
                assert_eq!(
                    frame_rows.len(),
                    target.frames.len(),
                    "full target call stack is required:\n{report}"
                );
                for (index, row) in frame_rows.iter().enumerate() {
                    assert_eq!(
                        row[0],
                        index.to_string(),
                        "target frames must retain root-to-active order"
                    );
                }
                let identity = alias(&text, "Hashes", "States", &target.code_identity.to_string());
                assert!(report.contains(&format!("target code_identity={identity}")));
                for (frame, row) in target.frames.iter().zip(frame_rows) {
                    let code = alias(
                        &text,
                        "Addresses",
                        "Hashes",
                        &frame.code_address.to_string(),
                    );
                    let owner = alias(&text, "Addresses", "Hashes", &frame.address.to_string());
                    let caller = alias(&text, "Addresses", "Hashes", &frame.caller.to_string());
                    let hash = alias(&text, "Hashes", "States", &frame.code_hash.to_string());
                    assert!(
                        row.contains(&code.as_str())
                            && row.contains(&owner.as_str())
                            && row.contains(&caller.as_str())
                            && row.contains(&hash.as_str())
                            && row.contains(&format!("B{}", frame.basic_block_index).as_str())
                    );
                    assert!(row.contains(&frame.stack_height.to_string().as_str()));
                    assert!(row.contains(&frame.is_static.to_string().as_str()));
                    assert!(row.contains(&format!("{:?}", frame.mode).as_str()));
                    assert!(row.contains(&format!("{:?}", frame.jump_history).as_str()));
                }
            }
        }
    }
}

#[test]
fn possible_log_records_keep_site_topics_payload_and_unknown_flag() {
    let mut world = World::new(Fork::Osaka, "test:log-report");
    world
        .insert(
            address(0x101),
            Account::from_hex("60ab5f53602a60015fa160015fa000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let analysis = analyze_world(world, entry(), ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    let outcome = analysis
        .outcomes()
        .iter()
        .find(|outcome| outcome.kind == OutcomeKind::Return)
        .unwrap();
    assert_eq!(outcome.store.possible_logs().len(), 2);
    let text = render::world::text(&analysis);
    let reports = numbered(section(&text, "Outcomes", "Call summaries"), "O");
    let report = reports[analysis
        .outcomes()
        .iter()
        .position(|candidate| std::ptr::eq(candidate, outcome))
        .unwrap()];
    assert!(report.contains("Possible logs") && report.contains("logs_unknown=false"));
    for (site, log) in outcome.store.possible_logs() {
        let owner = alias(&text, "Addresses", "Hashes", &site.address.to_string());
        assert!(report.contains(&owner) && report.contains(&format!("pc=0x{:x}", site.pc)));
        for topic in &log.topics {
            assert!(report.contains(&topic.to_string()));
        }
        assert!(
            report.contains("exact hex: 0xab"),
            "full LOG data must remain visible:\n{report}"
        );
    }
}

fn stack(values: &[Value]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn length(value: &Value) -> String {
    if let Some(constants) = value.constants()
        && constants.len() == 1
    {
        format!("{} bytes", constants.first().unwrap())
    } else {
        format!("{value} bytes")
    }
}

#[test]
fn state_details_and_transitions_preserve_original_graph_evidence() {
    let analysis = analyze("returndata-copy", ExecutionConfig::default());
    let text = render::world::text(&analysis);
    let reports = numbered(section(&text, "State details", "Transitions"), "S");
    assert_eq!(reports.len(), analysis.states().len());
    for (state, report) in analysis.states().iter().zip(reports) {
        let frame = state.entry.active();
        for (field, value) in [
            ("stack in", stack(&frame.stack)),
            ("stack out", stack(&state.exit_stack)),
            ("call value", frame.call_value.to_string()),
            ("calldata length", length(frame.calldata.len())),
            ("memory length", length(frame.memory.len())),
            ("returndata length", length(frame.returndata.len())),
        ] {
            assert!(
                report
                    .lines()
                    .any(|line| cells(line) == [field, value.as_str()]),
                "missing {field} for S{}:\n{report}",
                state.id
            );
        }
        assert!(
            report
                .lines()
                .any(|line| cells(line).first() == Some(&"pcs"))
        );
        assert!(
            report
                .lines()
                .any(|line| cells(line).first() == Some(&"jump history"))
        );
    }
    let transitions = section(&text, "Transitions", "Outcomes");
    let rows: Vec<_> = transitions
        .lines()
        .map(cells)
        .filter(|row| {
            row.first().is_some_and(|cell| {
                cell.starts_with('S') && cell[1..].bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .collect();
    assert_eq!(rows.len(), analysis.edges().len());
    for edge in analysis.edges() {
        assert!(rows.iter().any(|row| *row
            == [
                format!("S{}", edge.from).as_str(),
                format!("S{}", edge.to).as_str(),
                format!("{:?}", edge.kind).as_str()
            ]));
    }
}

#[test]
fn call_summary_reports_keep_cache_statistics_and_each_complete_relation() {
    let mut world = World::new(Fork::Osaka, "test:summary-report");
    for (owner, code) in [
        (
            0x101,
            "5f5f5f5f5f6102006207a120f1505f5f5f5f5f6102006207a120f15000",
        ),
        (0x200, "60015f5260205ff3"),
    ] {
        let mut account = Account::from_hex(code, Fork::Osaka).unwrap();
        account.balance = Value::constant(U256::from(1_000_000));
        world.insert(address(owner), account).unwrap();
    }
    for enabled in [true, false] {
        let analysis = analyze_world(
            world.clone(),
            entry(),
            ExecutionConfig {
                use_summaries: enabled,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(analysis.status(), Status::Converged);
        if enabled {
            assert!(
                analysis.summary_stats().hits > 0,
                "fixture must exercise reuse"
            );
        } else {
            assert!(analysis.summaries().is_empty());
        }
        let text = render::world::text(&analysis);
        let section = section(&text, "Call summaries", "Diagnostics");
        let stats = analysis.summary_stats();
        for (field, value) in [
            ("hits", stats.hits),
            ("misses", stats.misses),
            ("published", stats.published),
            ("rejected_incomplete", stats.rejected_incomplete),
            ("imported_states", stats.imported_states),
        ] {
            assert!(
                section.contains(&format!("{field}={value}")),
                "missing summary statistic {field}:\n{section}"
            );
        }
        assert!(section.contains(&format!("enabled={enabled}")));
        let reports = numbered(section, "summary#");
        assert_eq!(reports.len(), analysis.summaries().len());
        for (record, report) in analysis.summaries().iter().zip(reports) {
            assert!(
                report
                    .lines()
                    .next()
                    .unwrap()
                    .contains(&format!("source=S{}", record.source_state))
            );
            for (field, value) in [
                ("states", record.state_count),
                ("edges", record.edge_count),
                ("outputs", record.outputs.len()),
            ] {
                assert!(
                    report
                        .lines()
                        .any(|line| cells(line) == [field, value.to_string().as_str()])
                );
            }
            if let Some(hash) = record.input.code_hash {
                let hash = alias(&text, "Hashes", "States", &hash.to_string());
                assert!(
                    report
                        .lines()
                        .any(|line| cells(line) == ["code_hash", hash.as_str()])
                );
            }
            assert!(report.contains("reused_at="));
            for reused in &record.reused_at {
                assert!(
                    report
                        .lines()
                        .next()
                        .unwrap()
                        .contains(&format!("S{reused}"))
                );
            }
            let outputs: Vec<_> = report
                .lines()
                .map(cells)
                .filter(|row| {
                    row.first().is_some_and(|cell| {
                        !cell.is_empty() && cell.bytes().all(|byte| byte.is_ascii_digit())
                    })
                })
                .collect();
            assert_eq!(outputs.len(), record.outputs.len());
            for (output, row) in record.outputs.iter().zip(outputs) {
                let identity = alias(
                    &text,
                    "Hashes",
                    "States",
                    &output.store.code_identity().to_string(),
                );
                assert!(row.contains(&format!("{:?}", output.kind).as_str()));
                assert!(row.contains(&length(output.data.len()).as_str()));
                assert!(row.contains(&identity.as_str()));
            }
        }
    }
}
