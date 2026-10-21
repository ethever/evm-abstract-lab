//! Readable world reports preserve distinct paths, full identities and sparse byte facts.
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract_notation::{Symbol, normalize_subscripts};

use evm_abstract::{
    Address, Fork, U256,
    analysis::{EvmWorldAnalysis, ExecutionConfig, FrontierReason, OutcomeKind, Status},
    domain::{
        EvmValue,
        relational::{CheckResult, EvmValueQuery},
    },
    render,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld, Existence},
};

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn entry() -> EvmEntry<NumericValue> {
    EvmEntry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: (address(0x101)).into(),
            caller: (Address::ZERO).into(),
            value: EvmValue::<NumericValue>::constant(U256::ZERO),
            calldata: EvmByteArray::<NumericValue>::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
        },
    }
}

fn number(text: &str) -> U256 {
    U256::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

/// Read the same checked-in fixture used by the user's analyze command.
fn fixture(name: &str) -> EvmWorld<NumericValue> {
    let path = format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, json["provenance"].as_str().unwrap());
    for input in json["accounts"].as_array().unwrap() {
        let mut account = input["code"]
            .as_str()
            .map_or_else(EvmAccount::<NumericValue>::unknown, |code| {
                EvmAccount::<NumericValue>::from_hex(code, world.fork()).unwrap()
            });
        account.balance = input["balance"]
            .as_str()
            .map_or_else(EvmValue::<NumericValue>::top, |value| {
                EvmValue::<NumericValue>::constant(number(value))
            });
        // Match the CLI's partial-observation boundary instead of inheriting
        // the synthetic constructor's known nonce and account presence.
        account.nonce = input["nonce"]
            .as_str()
            .map_or_else(EvmValue::<NumericValue>::top, |value| {
                EvmValue::<NumericValue>::constant(number(value))
            });
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
                    EvmValue::<NumericValue>::constant(number(value.as_str().unwrap())),
                );
            }
        }
        world
            .insert(input["address"].as_str().unwrap().parse().unwrap(), account)
            .unwrap();
    }
    world
}

fn report_fixture(name: &str, config: ExecutionConfig) -> EvmWorldAnalysis<NumericValue> {
    native::run(
        &native::product(),
        AnalysisInput::new(fixture(name), entry()).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap()
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
            if suffix.is_empty()
                || !normalize_subscripts(suffix)
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            {
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
    // Wrapping adds padding and line breaks; retain every content character.
    fn compact(cell: &str) -> String {
        cell.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

    fn logical_rows(table: &str, headers: &[&str]) -> Vec<Vec<String>> {
        let mut lines = table.lines();
        if !lines.any(|line| line.split('|').map(str::trim).eq(headers.iter().copied())) {
            return Vec::new();
        }
        let separator: Vec<_> = lines
            .next()
            .expect("a table header retains its separator")
            .split('|')
            .map(str::trim)
            .collect();
        assert_eq!(separator.len(), headers.len());
        assert!(
            separator
                .iter()
                .all(|cell| { !cell.is_empty() && cell.chars().all(|character| character == '-') })
        );

        let mut rows: Vec<Vec<String>> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                break;
            }
            // Unlike cells(), retain the leading empty cell: a blank key
            // continues only the preceding row, in the same table and column.
            let components: Vec<_> = line.split('|').map(str::trim).collect();
            assert_eq!(components.len(), headers.len(), "{line}");
            if !components[0].is_empty() {
                rows.push(components.into_iter().map(compact).collect());
            } else {
                let row = rows
                    .last_mut()
                    .expect("a continuation must follow a keyed table row");
                for (cell, component) in row.iter_mut().zip(components) {
                    cell.push_str(&compact(component));
                }
            }
        }
        rows
    }

    let analysis = report_fixture("returndata-copy", ExecutionConfig::default());
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    let returned: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(
        !returned.is_empty(),
        "the successful copy must reach RETURN"
    );
    let domain = native::product_with(analysis.domain_spec());
    for outcome in returned {
        assert_eq!(outcome.data.len().singleton(), Some(U256::from(32)));
        let value = outcome
            .data
            .read_word_checked(
                &EvmValue::<NumericValue>::constant(U256::ZERO),
                domain.borrowed(),
            )
            .unwrap();
        // The caller entry joins failed and successful CALL results. The
        // RETURNDATACOPY range condition excludes the empty-data alternative;
        // the retained symbolic bytes and exit path together must still prove 1.
        let relations = &analysis.states()[outcome.state]
            .exit
            .as_ref()
            .expect("RETURN retains its completed payload")
            .relations;
        let limits = &analysis.domain_spec().relations();
        assert_eq!(
            relations.check(
                limits,
                native::product_with(native::spec(8, 4, 256).with_relations(limits.to_owned()))
                    .borrowed()
            ),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            relations.local_value(
                &value.symbolic_expression().unwrap(),
                limits,
                native::product_with(native::spec(8, 4, 256).with_relations(limits.to_owned()))
                    .borrowed()
            ),
            EvmValueQuery::<NumericValue>::Exact(U256::from(1))
        );
    }
    let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
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
    let mut checked_missing_continuation = false;
    for (index, (outcome, report)) in analysis.outcomes().iter().zip(outcomes).enumerate() {
        let header = cells(report.lines().next().unwrap());
        assert_eq!(header[0], format!("{}", Symbol::Outcome(index)));
        assert!(header.contains(&format!("{}", Symbol::State(outcome.state)).as_str()));
        assert!(header.contains(&format!("{:?}", outcome.kind).as_str()));
        let data = report
            .split_once("\n    Storage\n")
            .expect("each outcome retains its storage section")
            .0;
        assert!(data.contains(&format!("length={}", outcome.data.len())));
        assert!(data.contains(&format!("default={}", outcome.data.default_byte())));
        if let Some(bytes) = outcome.data.exact_bytes(native::product().borrowed())
            && !bytes.is_empty()
        {
            let hex = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert!(
                data.contains(&format!("exact hex: 0x{hex}")),
                "missing full returned bytes:\n{report}"
            );
            assert!(data.contains("0x001f") && data.contains("01"));
        }
        let byte_headers = ["Offset(s)", "Stored value / hex bytes"];
        let byte_rows = logical_rows(data, &byte_headers);
        for (offset, value) in outcome.data.stored_bytes() {
            if value.singleton().is_none() {
                let key = format!("0x{offset:04x}");
                let expected = compact(&value.to_string());
                let has_byte = |rows: &[Vec<String>]| {
                    rows.iter().any(|row| row[0] == key && row[1] == expected)
                };
                assert!(
                    has_byte(&byte_rows),
                    "missing abstract returned byte at {offset}:\n{report}"
                );
                if !checked_missing_continuation {
                    let physical_rows: Vec<_> = data.lines().collect();
                    let start = physical_rows
                        .iter()
                        .position(|line| line.split('|').next().unwrap().trim() == key)
                        .unwrap();
                    let continuation = physical_rows
                        .iter()
                        .enumerate()
                        .skip(start + 1)
                        .take_while(|(_, line)| line.split('|').next().unwrap().trim().is_empty())
                        .find(|(_, line)| {
                            line.split('|')
                                .nth(1)
                                .is_some_and(|cell| !cell.trim().is_empty())
                        })
                        .map(|(index, _)| index);
                    if let Some(continuation) = continuation {
                        // Other offsets and the default retain their components;
                        // none may supply a missing component for this byte.
                        let mut truncated = physical_rows;
                        truncated.remove(continuation);
                        assert!(
                            !has_byte(&logical_rows(&truncated.join("\n"), &byte_headers)),
                            "a missing continuation must fail at its own offset"
                        );
                        checked_missing_continuation = true;
                    }
                }
            }
        }
        let slots = logical_rows(
            section(report, "Storage", "Account observations"),
            &["Address", "Slot", "Value"],
        );
        assert_eq!(
            slots.len(),
            outcome.store.slots().len(),
            "all stored slots belong to this outcome"
        );
        for ((owner, slot), value) in outcome.store.slots() {
            let owner = alias(&text, "Addresses", "Hashes", &owner.to_string());
            let row = slots
                .iter()
                .find(|row| row[0] == owner && row[1] == format!("{slot:#x}"))
                .unwrap_or_else(|| panic!("missing stored slot in this outcome:\n{report}"));
            assert_eq!(
                row[2],
                compact(&value.to_string()),
                "missing stored value in this outcome:\n{report}"
            );
        }
        let accounts = section(report, "Account observations", "Possible logs");
        let rows = logical_rows(
            accounts,
            &[
                "Address",
                "Balance",
                "Nonce",
                "Existence",
                "Code hash",
                "Code size",
                "Created",
                "Pending destruction",
            ],
        );
        assert_eq!(
            rows.len(),
            outcome.store.addresses().len(),
            "all observed accounts belong to this outcome"
        );
        for address in outcome.store.addresses() {
            let reference = alias(&text, "Addresses", "Hashes", &address.to_string());
            let row = rows.iter().find(|row| row[0] == reference).unwrap();
            assert_eq!(
                row[1],
                compact(&outcome.store.read_balance(address).to_string())
            );
            assert_eq!(row[2], compact(&outcome.store.nonce(address).to_string()));
            assert_eq!(row[3], format!("{:?}", outcome.store.existence(address)));
            for (column, observed) in [
                (6, outcome.store.created_in_transaction(address)),
                (7, outcome.store.pending_destruction(address)),
            ] {
                assert_eq!(
                    row[column],
                    observed.map_or_else(|| "unknown".to_owned(), |flag| flag.to_string())
                );
            }
        }
    }
    assert!(
        checked_missing_continuation,
        "the fixture must exercise a wrapped stored byte and its negative control"
    );
}

#[test]
fn full_identity_legends_and_state_table_keep_proxy_code_and_storage_owners_distinct() {
    let analysis = report_fixture("proxy-storage", ExecutionConfig::default());
    let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
    let states = section(&text, "States", "State details");
    let rows: Vec<_> = states
        .lines()
        .map(cells)
        .filter(|row| {
            row.first().is_some_and(|cell| {
                cell.strip_prefix('σ').is_some_and(|suffix| {
                    !suffix.is_empty()
                        && normalize_subscripts(suffix)
                            .bytes()
                            .all(|byte| byte.is_ascii_digit())
                })
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
            .find(|row| row[0] == format!("{}", Symbol::State(state.id)))
            .unwrap();
        assert!(
            row.contains(&code.as_str())
                && row.contains(&owner.as_str())
                && row.contains(&caller.as_str())
                && row.contains(&hash.as_str())
        );
        assert!(row.contains(&format!("{}", Symbol::Block(frame.basic_block_index)).as_str()));
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
        text.contains(&analysis.world().fingerprint().unwrap().to_string()),
        "snapshot fingerprint must remain complete"
    );
}

#[test]
fn incomplete_reports_preserve_all_frontiers_and_every_target_frame() {
    for (name, config) in [
        (
            "missing-code",
            ExecutionConfig {
                max_call_depth: 1,
                ..ExecutionConfig::default()
            },
        ),
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
        let analysis = report_fixture(name, config);
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(!analysis.frontiers().is_empty());
        let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
        assert!(section(&text, "Analysis", "Snapshot").contains("Incomplete"));
        let reports = numbered(section(&text, "Frontiers", ""), "U");
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
                assert!(report.contains(&format!("{}", Symbol::State(from))));
            }
            if let Some(target) = &frontier.target {
                let frame_rows: Vec<_> = report
                    .lines()
                    .map(cells)
                    .filter(|row| {
                        row.first().is_some_and(|cell| {
                            cell.strip_prefix('f').is_some_and(|suffix| {
                                !suffix.is_empty()
                                    && normalize_subscripts(suffix)
                                        .bytes()
                                        .all(|byte| byte.is_ascii_digit())
                            })
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
                        Symbol::Frame(index).to_string(),
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
                            && row.contains(
                                &format!("{}", Symbol::Block(frame.basic_block_index)).as_str()
                            )
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
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "test:log-report");
    world
        .insert(
            address(0x101),
            EvmAccount::<NumericValue>::from_hex("60ab5f53602a60015fa160015fa000", Fork::Osaka)
                .unwrap(),
        )
        .unwrap();
    let analysis = native::run(
        &native::product(),
        AnalysisInput::new(world, entry()).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    let outcome = analysis
        .outcomes()
        .iter()
        .find(|outcome| outcome.kind == OutcomeKind::Return)
        .unwrap();
    assert_eq!(outcome.store.possible_logs().len(), 2);
    let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
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

fn stack(values: &[EvmValue<NumericValue>]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn length(value: &EvmValue<NumericValue>) -> String {
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
    let analysis = report_fixture("returndata-copy", ExecutionConfig::default());
    let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
    let reports = numbered(section(&text, "State details", "Transitions"), "σ");
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
                cell.strip_prefix('σ').is_some_and(|suffix| {
                    !suffix.is_empty()
                        && normalize_subscripts(suffix)
                            .bytes()
                            .all(|byte| byte.is_ascii_digit())
                })
            })
        })
        .collect();
    assert_eq!(rows.len(), analysis.edges().len());
    for edge in analysis.edges() {
        assert!(rows.iter().any(|row| *row
            == [
                format!("{}", Symbol::State(edge.from)).as_str(),
                format!("{}", Symbol::State(edge.to)).as_str(),
                format!("{:?}", edge.kind).as_str()
            ]));
    }
}

#[test]
fn call_summary_reports_keep_cache_statistics_and_each_complete_relation() {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "test:summary-report");
    for (owner, code) in [
        (
            0x101,
            "5f5f5f5f5f6102006207a120f1505f5f5f5f5f6102006207a120f15000",
        ),
        (0x200, "60015f5260205ff3"),
    ] {
        let mut account = EvmAccount::<NumericValue>::from_hex(code, Fork::Osaka).unwrap();
        account.balance = EvmValue::<NumericValue>::constant(U256::from(1_000_000));
        world.insert(address(owner), account).unwrap();
    }
    for enabled in [true, false] {
        let analysis = native::run(
            &native::product(),
            AnalysisInput::new(world.clone(), entry()).unwrap(),
            ExecutionConfig {
                use_summaries: enabled,
                ..ExecutionConfig::default()
            },
            &Control::default(),
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
        let text = render::world::text(&analysis, &native::product_with(analysis.domain_spec()));
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
        let reports = numbered(section, "summary");
        assert_eq!(reports.len(), analysis.summaries().len());
        for (record, report) in analysis.summaries().iter().zip(reports) {
            assert!(
                report
                    .lines()
                    .next()
                    .unwrap()
                    .contains(&format!("source={}", Symbol::State(record.source_state)))
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
                        .contains(&format!("{}", Symbol::State(*reused)))
                );
            }
            let outputs: Vec<_> = report
                .lines()
                .map(cells)
                .filter(|row| {
                    row.first().is_some_and(|cell| {
                        cell.rsplit_once(':')
                            .and_then(|(_, result)| result.strip_prefix('O'))
                            .is_some_and(|suffix| {
                                !suffix.is_empty()
                                    && normalize_subscripts(suffix)
                                        .bytes()
                                        .all(|byte| byte.is_ascii_digit())
                            })
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
