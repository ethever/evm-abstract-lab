//! Real HTTP discovery must preserve the complete fixed-snapshot transaction relation.

use alloy_primitives::{Address, B256, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{
        self, ExecutionConfig, FrontierReason, Limit, MachineEdgeKind, OutcomeKind, Status,
        WorldAnalysis,
    },
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprKind, SlotStateKind},
    },
    ssa,
    world::{
        Account, ByteArray, Entry, SnapshotIdentity,
        rpc::{self, AccountRequest, RpcBlock, RpcError, RpcFailureKind, RpcInput},
    },
};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn account(code: &str) -> Account {
    let mut account = Account::from_hex(code, Fork::Osaka).unwrap();
    account.balance = word(100);
    account.nonce = word(1);
    account.storage.insert(U256::ZERO, word(7));
    account
}

fn accounts(entries: &[(u64, &str)]) -> BTreeMap<Address, Account> {
    entries
        .iter()
        .map(|(number, code)| (address(*number), account(code)))
        .collect()
}

fn entry() -> Entry {
    Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: word(0),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    }
}

fn config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    }
}

fn call(op: u8, target: u16, value: u8) -> String {
    let value = if matches!(op, 0xf1 | 0xf2) {
        format!("60{value:02x}")
    } else {
        String::new()
    };
    format!("5f5f5f5f{value}61{target:04x}6207a120{op:02x}")
}

struct Server {
    endpoint: String,
    requests: Arc<Mutex<Vec<Json>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn new<'scope, 'env>(
        scope: &'scope thread::Scope<'scope, 'env>,
        accounts: BTreeMap<Address, Account>,
    ) -> Self {
        Self::with_code_failures(scope, accounts, BTreeSet::new())
    }

    fn with_code_failures<'scope, 'env>(
        scope: &'scope thread::Scope<'scope, 'env>,
        accounts: BTreeMap<Address, Account>,
        failed: BTreeSet<Address>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        scope.spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fake RPC accept failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: Json = serde_json::from_slice(&body).unwrap();
                observed.lock().unwrap().push(request.clone());
                let code_failure = request["method"] == "eth_getCode"
                    && request["params"][0]
                        .as_str()
                        .and_then(|text| text.parse::<Address>().ok())
                        .is_some_and(|address| failed.contains(&address));
                let response = if code_failure {
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32001,"message":"unavailable fixture state"}})
                } else {
                    response(&accounts, &request)
                };
                let bytes = serde_json::to_vec(&response).unwrap();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            stop,
        }
    }

    fn input(&self) -> RpcInput {
        let mut input = RpcInput::new(&self.endpoint, Fork::Osaka);
        input.block = RpcBlock::Hash(B256::repeat_byte(0x11));
        input.accounts.push(AccountRequest {
            address: address(0x101),
            slots: BTreeSet::from([U256::ZERO]),
        });
        input
    }

    fn code_requests(&self, target: Address) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request["method"] == "eth_getCode" && request["params"][0] == json!(target)
            })
            .count()
    }

    fn clear_requests(&self) {
        self.requests.lock().unwrap().clear();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn response(accounts: &BTreeMap<Address, Account>, request: &Json) -> Json {
    let method = request["method"].as_str().unwrap();
    let result = match method {
        "eth_chainId" => json!("0x1"),
        "eth_getBlockByHash" | "eth_getBlockByNumber" => {
            json!({"hash":B256::repeat_byte(0x11), "number":"0x2a", "parentHash": B256::repeat_byte(0x10), "timestamp":"0x1234", "miner": Address::repeat_byte(0x33), "mixHash": B256::repeat_byte(0x44), "gasLimit":"0x1c9c380", "baseFeePerGas":"0x7",
                "sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
                "stateRoot":B256::repeat_byte(0x11), "transactionsRoot":B256::repeat_byte(0x11), "receiptsRoot":B256::repeat_byte(0x11),
                "logsBloom":format!("0x{}", "00".repeat(256)),
                "difficulty":"0x0", "gasUsed":"0x0", "extraData":"0x",
                "nonce":"0x0000000000000000"})
        }
        _ => {
            let owner: Address = request["params"][0].as_str().unwrap().parse().unwrap();
            let account = accounts
                .get(&owner)
                .expect("unexpected account acquisition");
            let balance = account.balance.singleton().unwrap();
            let nonce = account.nonce.singleton().unwrap();
            let code = match &account.code {
                evm_abstract::world::Code::Runtime(program) => program.bytes().to_vec(),
                evm_abstract::world::Code::Delegation(target) => {
                    let mut bytes = vec![0xef, 0x01, 0x00];
                    bytes.extend_from_slice(target.as_slice());
                    bytes
                }
                evm_abstract::world::Code::Empty => Vec::new(),
                evm_abstract::world::Code::Unknown => panic!("RPC fixture code must be known"),
            };
            match method {
                "eth_getCode" => json!(format!("0x{}", hex::encode(&code))),
                "eth_getBalance" => json!(format!("{balance:#x}")),
                "eth_getTransactionCount" => json!(format!("{nonce:#x}")),
                "eth_getStorageAt" => {
                    let slot = U256::from_str_radix(
                        request["params"][1]
                            .as_str()
                            .unwrap()
                            .trim_start_matches("0x"),
                        16,
                    )
                    .unwrap();
                    let value = account
                        .storage
                        .get(&slot)
                        .and_then(AbstractValue::singleton)
                        .unwrap_or(U256::ZERO);
                    json!(format!("0x{value:064x}"))
                }
                _ => panic!("unexpected RPC method {method}"),
            }
        }
    };
    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
}

fn relations(analysis: &WorldAnalysis) -> BTreeSet<String> {
    // Independent executions allocate new private join selectors. Rename only
    // Fresh leaves, using one bijection across the complete outcome relation.
    // Preserve all graph edges, Input scopes, source namespaces and products.
    fn bind_fresh(value: &mut Json, leaves: &mut BTreeMap<(String, u64), u64>) {
        match value {
            Json::Array(values) => {
                for value in values {
                    bind_fresh(value, leaves);
                }
            }
            Json::Object(fields) => {
                if let Some(scope) = fields
                    .get("identity_scope")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
                {
                    for node in fields.get_mut("nodes").unwrap().as_array_mut().unwrap() {
                        if let Some(token) = node.get_mut("Fresh") {
                            let next = u64::try_from(leaves.len()).unwrap();
                            let renamed = *leaves
                                .entry((scope.clone(), token.as_u64().unwrap()))
                                .or_insert(next);
                            *token = Json::from(renamed);
                        }
                    }
                }
                for value in fields.values_mut() {
                    bind_fresh(value, leaves);
                }
            }
            _ => {}
        }
    }
    let mut leaves = BTreeMap::new();
    analysis
        .outcomes()
        .iter()
        .map(|outcome| {
            let mut value =
                serde_json::to_value((outcome.kind, &outcome.data, &outcome.store)).unwrap();
            bind_fresh(&mut value, &mut leaves);
            serde_json::to_string(&value).unwrap()
        })
        .collect()
}

fn compare(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
    acquired: &[Address],
) -> WorldAnalysis {
    let discovered = analysis::analyze_rpc(input, entry.clone(), config.clone()).unwrap();
    assert!(discovered.failures().is_empty());
    let discovered = discovered.into_analysis();
    assert_eq!(
        discovered.status(),
        Status::Converged,
        "{:?}",
        discovered.frontiers()
    );
    let stats = discovered.rpc_acquisition().unwrap();
    assert_eq!(stats.fetched_accounts, acquired);
    assert!(stats.failed_accounts.is_empty());
    let mut full_input = input.clone();
    full_input
        .accounts
        .extend(acquired.iter().map(|address| AccountRequest {
            address: *address,
            slots: BTreeSet::new(),
        }));
    let full_world = rpc::load(&full_input).unwrap();
    assert_eq!(
        discovered.world().fingerprint().unwrap(),
        full_world.fingerprint().unwrap()
    );
    assert_eq!(discovered.world().identity(), full_world.identity());
    let full = analysis::analyze_world(full_world, entry.clone(), config.clone()).unwrap();
    assert_eq!(relations(&discovered), relations(&full));
    let uncached = analysis::analyze_world(
        full.world().clone(),
        entry,
        ExecutionConfig {
            use_summaries: false,
            ..config
        },
    )
    .unwrap();
    assert_eq!(relations(&discovered), relations(&uncached));
    for result in [&discovered, &full, &uncached] {
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        ssa::build_world(result).unwrap().verify(result).unwrap();
        let checkpoint = result.states()[0]
            .entry
            .call_stack
            .root()
            .state
            .saved_store
            .state();
        // Fixed chain observations must keep their exact identity across outcomes,
        // regardless of the private selectors renamed by the relation comparison.
        for owner in result.world().accounts().keys() {
            let root = checkpoint.slot_root(*owner, false).unwrap();
            assert_eq!(
                root,
                discovered.states()[0]
                    .entry
                    .call_stack
                    .root()
                    .state
                    .saved_store
                    .state()
                    .slot_root(*owner, false)
                    .unwrap()
            );
            for outcome in result.outcomes() {
                if matches!(outcome.kind, OutcomeKind::Failure | OutcomeKind::Revert) {
                    assert_eq!(outcome.store.slot_root(*owner, false).unwrap(), root);
                }
            }
        }
        for summary in result.summaries() {
            assert_eq!(
                summary.input.world_fingerprint,
                result.world().fingerprint().unwrap()
            );
        }
    }
    discovered
}

#[test]
fn discovery_replays_unknown_slot_alias_writes_before_delegatecall() {
    thread::scope(|scope| {
        // Unknown calldata chooses an SSTORE alias before the missing callee is found.
        let caller = format!("60095f3555{}5000", call(0xf4, 0x200, 0));
        let server = Server::new(
            scope,
            accounts(&[(0x101, &caller), (0x200, "5f5460015500")]),
        );
        let mut entry = entry();
        entry.environment.calldata = ByteArray::unknown();
        let result = compare(&server.input(), entry, config(), &[address(0x200)]);
        assert_eq!(result.rpc_acquisition().unwrap().rounds, 2);
        for outcome in result
            .outcomes()
            .iter()
            .filter(|o| o.kind == OutcomeKind::Return)
        {
            let value = outcome
                .store
                .read(address(0x101), &word(0), Domain::default());
            assert!(value.contains(U256::from(7)));
            assert!(value.contains(U256::from(9)));
        }
        assert!(
            result
                .world()
                .account(address(0x200))
                .unwrap()
                .storage_unknown
        );
        assert!(
            result
                .world()
                .account(address(0x200))
                .unwrap()
                .storage
                .is_empty()
        );
    });
}

#[test]
fn late_callee_discovery_replays_prior_selfdestruct_and_value_transfers() {
    thread::scope(|scope| {
        let caller = format!("{}50{}5000", call(0xf1, 0x200, 5), call(0xf1, 0x300, 7));
        let server = Server::new(
            scope,
            accounts(&[(0x101, &caller), (0x200, "610300ff"), (0x300, "00")]),
        );
        let mut input = server.input();
        input.accounts.push(AccountRequest {
            address: address(0x200),
            slots: BTreeSet::new(),
        });
        let result = compare(&input, entry(), config(), &[address(0x300)]);
        assert!(result.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome
                    .store
                    .read_balance(address(0x300))
                    .contains(U256::from(212))
                && outcome
                    .store
                    .read_balance(address(0x200))
                    .contains(U256::ZERO)
                && outcome
                    .store
                    .read_balance(address(0x101))
                    .contains(U256::from(88))
        }));
    });
}

#[test]
fn nested_discovery_rebuilds_ancestor_and_root_rollback_checkpoints() {
    thread::scope(|scope| {
        for root_reverts in [false, true] {
            let ending = if root_reverts { "5f5ffd" } else { "00" };
            let caller = format!("60035f5560095f5d{}50{ending}", call(0xf1, 0x200, 5));
            let child = format!("60045f5560085f5d{}505f5ffd", call(0xf1, 0x300, 2));
            let server = Server::new(
                scope,
                accounts(&[(0x101, &caller), (0x200, &child), (0x300, "60055f5500")]),
            );
            let result = compare(
                &server.input(),
                entry(),
                config(),
                &[address(0x200), address(0x300)],
            );
            assert_eq!(result.rpc_acquisition().unwrap().rounds, 3);
            assert_eq!(
                result.world().identity(),
                &SnapshotIdentity::Chain {
                    chain_id: U256::from(1),
                    block_hash: B256::repeat_byte(0x11),
                }
            );
            let domain = Domain::default();
            let checkpoint = result.states()[0]
                .entry
                .call_stack
                .root()
                .state
                .saved_store
                .state();
            let initial_read = checkpoint.read(address(0x101), &word(0), domain);
            assert_eq!(initial_read.numeric(), word(7).numeric());
            assert_eq!(initial_read.provenance(), word(7).provenance());
            assert_eq!(
                checkpoint.read(address(0x101), &word(0), domain),
                initial_read
            );
            let ExprKind::SlotRead { state, key } = initial_read.expression().unwrap().kind()
            else {
                panic!("RPC initial slot must be tied to its fixed snapshot");
            };
            assert_eq!(key.as_constant(), Some(U256::ZERO));
            assert_eq!(*state, checkpoint.slot_root(address(0x101), false).unwrap());
            let SlotStateKind::Input { source, bindings } = state.kind() else {
                panic!("RPC initial read must retain chain observations");
            };
            assert!(source.token().as_constant().is_some());
            assert_eq!(source.namespace(), U256::from(0x101) << 8);
            assert_eq!(bindings.len(), 1);
            assert_eq!(bindings[0].index, U256::ZERO);
            assert_eq!(bindings[0].value.as_constant(), Some(U256::from(7)));
            assert_eq!(&bindings[0].numeric, word(7).numeric());
            let reverts: Vec<_> = result
                .edges()
                .iter()
                .filter(|edge| {
                    edge.kind == MachineEdgeKind::Revert
                        && result.states()[edge.from].key.frames.len() == 2
                        && result.states()[edge.to].key.frames.len() == 1
                })
                .collect();
            assert!(!reverts.is_empty());
            for edge in reverts {
                let child = &result.states()[edge.from];
                let saved = child.entry.active().saved_store.state();
                let resumed = &result.states()[edge.to].entry;
                assert_eq!(&resumed.store, saved);
                assert_eq!(
                    resumed.store.read(address(0x101), &word(0), domain),
                    word(3)
                );
                assert_eq!(
                    resumed
                        .store
                        .read_transient(address(0x101), &word(0), domain),
                    word(9)
                );
                for owner in [address(0x200), address(0x300)] {
                    assert_eq!(
                        resumed.store.slot_root(owner, false),
                        checkpoint.slot_root(owner, false)
                    );
                    assert_eq!(
                        resumed.store.read_transient(owner, &word(0), domain),
                        word(0)
                    );
                    assert_eq!(resumed.store.read_balance(owner), word(100));
                }
                assert_eq!(resumed.active().stack.last().unwrap(), &word(0));
                let receipt = child.execution_evidence().unwrap();
                assert!(receipt.is_current());
                assert!(
                    receipt
                        .successors()
                        .contains(&(edge.to, MachineEdgeKind::Revert))
                );
            }
            let deeper_returns: Vec<_> = result
                .edges()
                .iter()
                .filter(|edge| {
                    edge.kind == MachineEdgeKind::Return
                        && result.states()[edge.from].key.frames.len() == 3
                        && result.states()[edge.to].key.frames.len() == 2
                })
                .collect();
            assert!(!deeper_returns.is_empty());
            for edge in deeper_returns {
                let grandchild = &result.states()[edge.from];
                let saved = grandchild.entry.active().saved_store.state();
                assert_eq!(saved.read(address(0x200), &word(0), domain), word(4));
                assert_eq!(
                    saved.read_transient(address(0x200), &word(0), domain),
                    word(8)
                );
                assert_eq!(
                    saved.read_transient(address(0x101), &word(0), domain),
                    word(9)
                );
                assert_eq!(
                    grandchild
                        .exit
                        .as_ref()
                        .unwrap()
                        .store
                        .read(address(0x300), &word(0), domain),
                    word(5)
                );
            }
            let expected_kind = if root_reverts {
                OutcomeKind::Revert
            } else {
                OutcomeKind::Return
            };
            let outcomes: Vec<_> = result
                .outcomes()
                .iter()
                .filter(|o| o.kind == expected_kind)
                .collect();
            assert!(!outcomes.is_empty());
            for outcome in outcomes {
                assert_eq!(outcome.store.read_balance(address(0x101)), word(100));
                assert_eq!(outcome.store.read_balance(address(0x200)), word(100));
                assert_eq!(outcome.store.read_balance(address(0x300)), word(100));
                let actual = outcome.store.read(address(0x101), &word(0), domain);
                let expected = if root_reverts {
                    &initial_read
                } else {
                    &word(3)
                };
                assert_eq!(actual.numeric(), expected.numeric());
                assert_eq!(actual.provenance(), expected.provenance());
                assert_eq!(&actual, expected);
                assert_eq!(
                    outcome
                        .store
                        .read_transient(address(0x200), &word(0), Domain::default()),
                    word(0)
                );
                assert!(outcome.store.transient_slots().is_empty());
                for owner in [address(0x200), address(0x300)] {
                    assert_eq!(
                        outcome.store.slot_root(owner, false),
                        checkpoint.slot_root(owner, false)
                    );
                }
            }
            let checkpoint = result.states()[0]
                .entry
                .call_stack
                .root()
                .state
                .saved_store
                .state();
            assert_eq!(checkpoint.read_balance(address(0x300)), word(100));
            assert_eq!(
                checkpoint.read(address(0x101), &word(0), domain),
                initial_read
            );
        }
    });
}

fn push2(bytes: &mut Vec<u8>, number: usize) {
    bytes.extend([0x61, (number >> 8) as u8, number as u8]);
}

fn deploy_then_call(runtime: &[u8], late_target: u16) -> Vec<u8> {
    let mut initcode = Vec::new();
    push2(&mut initcode, runtime.len());
    push2(&mut initcode, 13);
    initcode.extend([0x5f, 0x39]);
    push2(&mut initcode, runtime.len());
    initcode.extend([0x5f, 0xf3]);
    initcode.extend(runtime);
    let mut factory = Vec::new();
    push2(&mut factory, initcode.len());
    push2(&mut factory, 0);
    factory.extend([0x5f, 0x39]);
    push2(&mut factory, initcode.len());
    factory.extend([0x5f, 0x5f, 0xf0]);
    // Retain CREATE's address in slot zero, then CALL the deployed runtime.
    factory.extend(hex::decode("805f55505f5f5f5f5f5f546207a120f150").unwrap());
    factory.extend(hex::decode(call(0xf1, late_target, 0)).unwrap());
    factory.extend([0x50, 0x00]);
    let offset = factory.len();
    factory[4] = (offset >> 8) as u8;
    factory[5] = offset as u8;
    factory.extend(initcode);
    factory
}

#[test]
fn replay_keeps_create_runtime_overlay_and_never_refetches_its_initial_code() {
    thread::scope(|scope| {
        let runtime = hex::decode("60095f5500").unwrap();
        let factory = hex::encode(deploy_then_call(&runtime, 0x200));
        let created = address(0x101).create(1);
        let mut fixtures = accounts(&[(0x101, &factory), (0x200, "00")]);
        fixtures.insert(created, Account::absent());
        fixtures.insert(Address::ZERO, Account::absent());
        let server = Server::new(scope, fixtures);
        let mut input = server.input();
        for address in [created, Address::ZERO] {
            input.accounts.push(AccountRequest {
                address,
                slots: BTreeSet::new(),
            });
        }
        let result = analysis::analyze_rpc(&input, entry(), config()).unwrap();
        let result = result.into_analysis();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert_eq!(
            result.rpc_acquisition().unwrap().fetched_accounts,
            [address(0x200)]
        );
        assert_eq!(server.code_requests(created), 1);
        assert!(result.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome.store.raw_account_code(created).as_deref() == Some(runtime.as_slice())
                && outcome
                    .store
                    .read(created, &word(0), Domain::default())
                    .contains(U256::from(9))
        }));
        let full_world = result.world().clone();
        let full = analysis::analyze_world(full_world, entry(), config()).unwrap();
        assert_eq!(relations(&result), relations(&full));
        ssa::build_world(&result).unwrap().verify(&result).unwrap();
    });
}

#[test]
fn discovery_follows_exactly_one_7702_pointer_without_loading_a_second_marker_target() {
    thread::scope(|scope| {
        let caller = format!("{}5000", call(0xf1, 0x200, 0));
        let delegated = format!("ef0100{}", hex::encode(address(0x300)));
        let second_marker = format!("ef0100{}", hex::encode(address(0x400)));
        let server = Server::new(
            scope,
            accounts(&[
                (0x101, &caller),
                (0x200, &delegated),
                (0x300, &second_marker),
                (0x400, "00"),
            ]),
        );
        let result = analysis::analyze_rpc(&server.input(), entry(), config())
            .unwrap()
            .into_analysis();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert_eq!(
            result.rpc_acquisition().unwrap().fetched_accounts,
            [address(0x200), address(0x300)]
        );
        assert_eq!(server.code_requests(address(0x200)), 1);
        assert_eq!(server.code_requests(address(0x300)), 1);
        assert_eq!(server.code_requests(address(0x400)), 0);
        assert!(result.world().account(address(0x400)).is_none());
        ssa::build_world(&result).unwrap().verify(&result).unwrap();
    });
}

#[test]
fn finite_targets_are_projected_to_low160_and_acquired_once() {
    thread::scope(|scope| {
        // CALLVALUE supplies two 256-bit observations with one account address.
        let caller = "5f5f5f5f5f346207a120f1505f5f5f5f5f346207a120f15000";
        let server = Server::new(scope, accounts(&[(0x101, caller), (0x200, "00")]));
        let mut entry = entry();
        let low = U256::from(0x200);
        let high = low | (U256::from(1) << 200);
        entry.environment.value = Domain::default().join(
            &AbstractValue::constant(low),
            &AbstractValue::constant(high),
        );
        let result = analysis::analyze_rpc(&server.input(), entry.clone(), config())
            .unwrap()
            .into_analysis();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert_eq!(
            result.rpc_acquisition().unwrap().fetched_accounts,
            [address(0x200)]
        );
        assert_eq!(server.code_requests(address(0x200)), 1);
        assert_eq!(result.rpc_acquisition().unwrap().rounds, 2);
        let full = analysis::analyze_world(result.world().clone(), entry, config()).unwrap();
        assert_eq!(relations(&result), relations(&full));
    });
}

#[test]
fn final_snapshot_binds_reused_nested_call_certificates() {
    thread::scope(|scope| {
        let caller = format!("{}50{}5000", call(0xfa, 0x200, 0), call(0xfa, 0x200, 0));
        let child = format!("{}5000", call(0xfa, 0x300, 0));
        let server = Server::new(
            scope,
            accounts(&[(0x101, &caller), (0x200, &child), (0x300, "00")]),
        );
        let result = compare(
            &server.input(),
            entry(),
            config(),
            &[address(0x200), address(0x300)],
        );
        assert!(
            result.summary_stats().hits > 0,
            "{:?}",
            result.summary_stats()
        );
        assert!(!result.summaries().is_empty());
        for summary in result.summaries() {
            assert_eq!(
                summary.input.world_fingerprint,
                result.world().fingerprint().unwrap()
            );
            assert_eq!(&summary.input.snapshot, result.world().identity());
        }
    });
}

#[test]
fn call_depth_frontier_does_not_acquire_an_unenterable_callee() {
    thread::scope(|scope| {
        let caller = format!("{}5000", call(0xf1, 0x200, 0));
        let server = Server::new(scope, accounts(&[(0x101, &caller), (0x200, "00")]));
        let bounded = ExecutionConfig {
            max_call_depth: 1,
            ..config()
        };
        let result = analysis::analyze_rpc(&server.input(), entry(), bounded).unwrap();
        assert!(result.failures().is_empty());
        let result = result.into_analysis();
        assert_eq!(result.status(), Status::Incomplete);
        assert!(
            result
                .frontiers()
                .iter()
                .any(|frontier| { matches!(frontier.reason, FrontierReason::CallDepth) })
        );
        assert!(
            !result
                .frontiers()
                .iter()
                .any(|frontier| { matches!(frontier.reason, FrontierReason::MissingCode(_)) })
        );
        assert_eq!(result.rpc_acquisition().unwrap().rounds, 1);
        assert!(
            result
                .rpc_acquisition()
                .unwrap()
                .fetched_accounts
                .is_empty()
        );
        assert_eq!(server.code_requests(address(0x200)), 0);
        assert!(result.world().account(address(0x200)).is_none());
    });
}

#[test]
fn known_insufficient_balance_does_not_acquire_a_failing_value_call() {
    thread::scope(|scope| {
        for op in [0xf1, 0xf2] {
            let caller = format!("{}5000", call(op, 0x200, 101));
            let server = Server::new(scope, accounts(&[(0x101, &caller), (0x200, "00")]));
            let result = analysis::analyze_rpc(&server.input(), entry(), config()).unwrap();
            assert!(result.failures().is_empty());
            let result = result.into_analysis();
            assert_eq!(result.status(), Status::Converged);
            assert!(result.frontiers().is_empty());
            assert_eq!(result.rpc_acquisition().unwrap().rounds, 1);
            assert!(
                result
                    .rpc_acquisition()
                    .unwrap()
                    .fetched_accounts
                    .is_empty()
            );
            assert_eq!(server.code_requests(address(0x200)), 0);
            assert!(result.world().account(address(0x200)).is_none());
            assert!(result.outcomes().iter().any(|outcome| {
                outcome.kind == OutcomeKind::Return
                    && outcome.store.read_balance(address(0x101)) == word(100)
            }));
            ssa::build_world(&result).unwrap().verify(&result).unwrap();
        }
    });
}

#[test]
fn failed_and_acquired_accounts_remain_paired_when_the_next_round_stops_early() {
    thread::scope(|scope| {
        // Source order enters C before B; address order tries failed B before
        // successful C. The bounded replay cannot reach B's CALL instruction.
        let caller = format!("{}50{}5000", call(0xf1, 0x300, 0), call(0xf1, 0x200, 0));
        // Valid-size observed code makes the refined snapshot costly to lift;
        // execution itself stops at the first byte and adds no callee effects.
        let leaf = "00".repeat(24_576);
        let server = Server::with_code_failures(
            scope,
            accounts(&[(0x101, &caller), (0x200, "00"), (0x300, &leaf)]),
            BTreeSet::from([address(0x200)]),
        );
        let input = server.input();
        // Measure the discovery-mode first round, keeping acquisition disabled
        // by its own account cap. Offline execution now covers missing code
        // through abstract interactions and is a different execution policy.
        let mut first_input = input.clone();
        first_input.max_accounts = 1;
        let first_round = analysis::analyze_rpc(&first_input, entry(), config())
            .unwrap()
            .into_analysis();
        assert_eq!(first_round.status(), Status::Incomplete);
        for limit in [Limit::States, Limit::Work] {
            let mut bounded = config();
            match limit {
                Limit::States => bounded.analysis.max_states = first_round.states().len() + 1,
                Limit::Work => bounded.max_work = first_round.work() * 2,
                _ => unreachable!(),
            }
            server.clear_requests();
            let result = analysis::analyze_rpc(&input, entry(), bounded.clone()).unwrap();
            assert_eq!(result.failures().len(), 1);
            let original = &result.failures()[0];
            assert!(matches!(original, RpcError::Remote { code: -32001, .. }));
            let evidence = original.failure();
            let result = result.into_analysis();
            assert_eq!(result.status(), Status::Incomplete);
            let acquisition = result.rpc_acquisition().unwrap();
            assert_eq!(acquisition.rounds, 2);
            assert_eq!(acquisition.fetched_accounts, [address(0x300)]);
            assert_eq!(acquisition.failed_accounts, [address(0x200)]);
            assert_eq!(acquisition.failures.len(), 1);
            assert_eq!(acquisition.failures[0].address, address(0x200));
            assert_eq!(acquisition.failures[0].failure, evidence);
            assert_eq!(acquisition.failures[0].failure.kind, RpcFailureKind::Remote);
            assert_eq!(
                acquisition.failures[0].failure.context.method,
                "eth_getCode"
            );
            assert_eq!(
                acquisition.failures[0].failure.context.account,
                Some(address(0x200))
            );
            assert_eq!(
                acquisition.failures[0].failure.context.chain_id,
                Some(U256::from(1))
            );
            assert_eq!(
                acquisition.failures[0].failure.context.block_hash,
                Some(B256::repeat_byte(0x11))
            );
            assert_eq!(server.code_requests(address(0x200)), 1);
            assert_eq!(server.code_requests(address(0x300)), 1);
            assert!(result.world().account(address(0x200)).is_none());
            assert!(result.world().account(address(0x300)).is_some());
            assert!(!result.frontiers().iter().any(|frontier| {
                matches!(frontier.reason, FrontierReason::RpcAcquisition { address: failed, .. } if failed == address(0x200))
            }), "failed CALL was unexpectedly reached: {:?}", result.frontiers());
            match limit {
                Limit::States => {
                    assert_eq!(result.states().len(), 1);
                    assert_eq!(acquisition.states_created, bounded.analysis.max_states);
                    assert!(result.frontiers().iter().any(|frontier| matches!(
                        frontier.reason,
                        FrontierReason::Budget(Limit::States)
                    )));
                }
                Limit::Work => {
                    assert!(result.states().is_empty());
                    assert!(
                        result
                            .frontiers()
                            .iter()
                            .any(|frontier| matches!(frontier.reason, FrontierReason::Work))
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(result.config().max_work, bounded.max_work);
            assert_eq!(
                result.config().analysis.max_states,
                bounded.analysis.max_states
            );
            assert!(result.work() <= bounded.max_work);
            assert!(acquisition.states_created <= bounded.analysis.max_states);
        }
    });
}

#[test]
fn discovery_rounds_share_work_transfer_and_state_limits_without_rewriting_config() {
    thread::scope(|scope| {
        let caller = format!("{}5000", call(0xf1, 0x200, 0));
        let child = format!("{}5000", call(0xf1, 0x300, 0));
        let server = Server::new(
            scope,
            accounts(&[(0x101, &caller), (0x200, &child), (0x300, "00")]),
        );
        let input = server.input();
        let full = analysis::analyze_rpc(&input, entry(), config())
            .unwrap()
            .into_analysis();
        assert_eq!(full.status(), Status::Converged);
        let cumulative_states = full.rpc_acquisition().unwrap().states_created;
        assert!(cumulative_states > full.states().len());
        assert!(full.rpc_acquisition().unwrap().rounds > 1);
        assert_eq!(
            full.rpc_acquisition().unwrap().fetched_accounts,
            [address(0x200), address(0x300)]
        );
        let final_only = analysis::analyze_world(full.world().clone(), entry(), config()).unwrap();
        assert!(full.transfers() > final_only.transfers());
        assert!(full.work() > final_only.work());
        for limit in [Limit::Work, Limit::Transfers, Limit::States] {
            let mut bounded = config();
            match limit {
                Limit::Work => bounded.max_work = full.work() - 1,
                Limit::Transfers => bounded.analysis.max_transfers = full.transfers() - 1,
                Limit::States => bounded.analysis.max_states = cumulative_states - 1,
                _ => unreachable!(),
            }
            server.clear_requests();
            let mut result = analysis::analyze_rpc(&input, entry(), bounded.clone())
                .unwrap()
                .into_analysis();
            if limit == Limit::Work && result.status() == Status::Converged {
                // A final optional memo fee can be declined without stopping
                // execution. Preserve that complete multi-round control, then
                // interrupt below its actual paid prefix, not a guessed fee.
                assert!(result.frontiers().is_empty());
                assert!(result.work() < full.work());
                assert_eq!(result.transfers(), full.transfers());
                assert_eq!(result.states().len(), full.states().len());
                assert_eq!(
                    serde_json::to_value(result.rpc_acquisition()).unwrap(),
                    serde_json::to_value(full.rpc_acquisition()).unwrap()
                );
                assert_eq!(result.world().accounts(), full.world().accounts());
                assert_eq!(
                    serde_json::to_value(result.config()).unwrap(),
                    serde_json::to_value(&bounded).unwrap()
                );
                ssa::build_world(&result).unwrap().verify(&result).unwrap();
                let work = result.work().checked_sub(1).unwrap();
                assert!(work > 0 && work < bounded.max_work);
                bounded.max_work = work;
                server.clear_requests();
                result = analysis::analyze_rpc(&input, entry(), bounded.clone())
                    .unwrap()
                    .into_analysis();
            }
            if limit == Limit::Work {
                // The refusal must follow acquisition, not replace this test
                // with a zero-budget interruption before discovery begins.
                assert_eq!(
                    result.rpc_acquisition().unwrap().rounds,
                    full.rpc_acquisition().unwrap().rounds
                );
                assert_eq!(
                    result.rpc_acquisition().unwrap().fetched_accounts,
                    full.rpc_acquisition().unwrap().fetched_accounts
                );
                assert_eq!(result.world().accounts(), full.world().accounts());
            }
            assert_eq!(result.status(), Status::Incomplete, "budget {limit:?}");
            assert_eq!(
                serde_json::to_value(result.config()).unwrap(),
                serde_json::to_value(&bounded).unwrap()
            );
            assert_eq!(result.config().max_work, bounded.max_work);
            assert_eq!(
                result.config().analysis.max_transfers,
                bounded.analysis.max_transfers
            );
            assert_eq!(
                result.config().analysis.max_states,
                bounded.analysis.max_states
            );
            assert!(result.work() <= bounded.max_work);
            assert!(result.transfers() <= bounded.analysis.max_transfers);
            assert!(
                result.rpc_acquisition().unwrap().states_created <= bounded.analysis.max_states
            );
            assert!(
                result
                    .frontiers()
                    .iter()
                    .any(|frontier| match &frontier.reason {
                        FrontierReason::Work | FrontierReason::SummaryWork => limit == Limit::Work,
                        FrontierReason::Budget(actual) => *actual == limit,
                        _ => false,
                    }),
                "limit {limit:?}: {:?}",
                result.frontiers()
            );
        }
    });
}
