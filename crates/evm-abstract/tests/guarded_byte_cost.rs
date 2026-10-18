//! End-to-end probe for guarded operand facts at the unchanged 20M work policy.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{Config, ExecutionConfig, OutcomeKind, Status, WorldAnalysis, analyze_world},
    domain::{
        AbstractValue, Domain,
        symbolic::{ByteStateKind, ExprId},
    },
    world::{Account, ByteArray, Entry, World},
};
use serde_json::json;

const WORK_LIMIT: usize = 20_000_000;
const JOIN_PC: usize = 21;
const MLOAD_PC: usize = 23;
// Unknown CALLDATALOAD(0) chooses SLOAD(0) or SLOAD(1). Each branch writes
// its own source to memory[0], then jumps to one shared MLOAD/RETURN block.
const CODE: &str = "5f35600c575f545f526015565b6001545f526015565b5f515f5260205ff3";

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn analyze(max_work: usize) -> WorldAnalysis {
    let owner = Address::repeat_byte(0x11);
    let mut account = Account::from_hex(CODE, Fork::Osaka).unwrap();
    // These two observations are the only initial storage guarantees. Other
    // slots and account balances remain unknown; calldata is wholly unknown.
    account.storage_unknown = true;
    account.storage.insert(U256::ZERO, word(7));
    account.storage.insert(U256::ONE, word(7));
    account.balance = AbstractValue::top();
    account.nonce = AbstractValue::top();
    let mut world = World::new(Fork::Osaka, "guarded-byte-cost:two-observed-slots");
    world.insert(owner, account).unwrap();
    let entry = Entry::concrete(
        owner,
        Address::repeat_byte(0x22),
        word(0),
        ByteArray::unknown(),
    );
    analyze_world(
        world,
        entry,
        ExecutionConfig {
            analysis: Config {
                // Force both branch edges into the same structural state.
                context_depth: 0,
                ..Config::default()
            },
            max_work,
            ..ExecutionConfig::default()
        },
    )
    .unwrap()
}

fn written_source(state: &evm_abstract::domain::symbolic::ByteStateId) -> &ExprId {
    let ByteStateKind::WriteWord { offset, value, .. } = state.kind() else {
        panic!("expected a branch-local word write: {state:?}");
    };
    assert_eq!(offset.as_constant(), Some(U256::ZERO));
    assert!(value.as_constant().is_none(), "source must retain its term");
    value
}

fn evidence_and_outputs(analysis: &WorldAnalysis) {
    eprintln!(
        "guarded-byte-cost {}",
        json!({
            "phase": "analysis",
            "code": CODE,
            "join_pc": JOIN_PC,
            "mload_pc": MLOAD_PC,
            "status": analysis.status(),
            "work": analysis.work(),
            "config": analysis.config(),
            "domain_spec": analysis.domain_spec(),
            "transfers": analysis.transfers(),
            "frontiers": analysis.frontiers(),
            "outcomes": analysis.outcomes().len(),
        })
    );
    let join = analysis
        .states()
        .iter()
        .find(|state| {
            state.program().is_some_and(|program| {
                program.blocks()[state.active().basic_block_index].start_pc == JOIN_PC
            })
        })
        .expect("analysis must reach the real shared MLOAD block");
    let memory = &join.entry.active().memory;
    let serialized = serde_json::to_value(memory).unwrap();
    let conditional = serialized["conditional"].as_array().unwrap();
    eprintln!(
        "guarded-byte-cost {}",
        json!({
            "phase": "joined_memory",
            "state_id": join.id,
            "incoming_edges": analysis.edges().iter().filter(|edge| edge.to == join.id).count(),
            "root_work": memory.state_root().work_size(),
            "memory_work": memory.work_size(),
            // This public serialization exposes the actual retained scopes.
            // word_numeric_work itself is crate-private; no accessor is added.
            "memory": &serialized,
            "top_value_work": AbstractValue::top().work_size(),
        })
    );
    assert!(!conditional.is_empty(), "probe missed conditional metadata");
    let ByteStateKind::Ite {
        then_state,
        else_state,
        ..
    } = memory.state_root().kind()
    else {
        panic!("probe must retain both source snapshots");
    };
    let sources = [written_source(then_state), written_source(else_state)];
    assert_ne!(sources[0], sources[1]);
    for source in sources {
        assert!(
            !memory.symbolic_facts().any(|(key, _)| key == source),
            "a one-sided source guarantee escaped its scope"
        );
        let serialized_source = serde_json::to_value(source).unwrap();
        assert!(
            conditional.iter().any(|branch| {
                ["then_facts", "else_facts"].into_iter().any(|side| {
                    branch[side]["facts"].as_array().is_some_and(|facts| {
                        facts
                            .iter()
                            .any(|fact| fact["expression"] == serialized_source)
                    })
                })
            }),
            "missing guarded source fact"
        );
    }
    assert_eq!(
        analysis
            .edges()
            .iter()
            .filter(|edge| edge.to == join.id)
            .count(),
        2,
        "probe must exercise a real two-way EVM join"
    );
    assert!(analysis.work() <= analysis.config().max_work);
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "frontiers={:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    let domain = Domain::from_spec(analysis.domain_spec());
    let returns: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(outcome.data.len().singleton(), Some(U256::from(32)));
        assert_eq!(
            outcome
                .data
                .read_word_checked(&word(0), domain)
                .unwrap()
                .singleton(),
            Some(U256::from(7)),
            "both source words are concretely 7"
        );
    }
}

#[test]
fn guarded_evm_word_sources_converge_at_twenty_million_work() {
    let analysis = analyze(WORK_LIMIT);
    assert_eq!(analysis.config().max_work, WORK_LIMIT);
    evidence_and_outputs(&analysis);
}

#[test]
#[ignore = "root-only diagnostic control; does not satisfy the 20M regression"]
fn guarded_evm_word_sources_larger_budget_semantic_control() {
    evidence_and_outputs(&analyze(200_000_000));
}
