//! Storage reuse distinguishes writes with equal numeric caches and new symbolic roots.

use std::collections::BTreeMap;

use evm_abstract::{
    Address, Fork, U256,
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, ExprKind, SlotStateKind},
    },
    world::{Account, Store, World},
};

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn partial_world() -> (World, Address) {
    let address = Address::repeat_byte(0x31);
    let mut account = Account::unknown();
    account.storage.insert(U256::from(4), AbstractValue::top());
    let mut world = World::new(Fork::Osaka, "initial storage dependencies");
    world.insert(address, account).unwrap();
    (world, address)
}

#[test]
fn strong_top_write_changes_reuse_state_work_and_symbolic_json() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let top = AbstractValue::top();
    let initial = Store::new(&world);
    let initial_root = initial.slot_root(address, false).unwrap();
    let initial_snapshot = initial.snapshot();
    let initial_json = serde_json::to_value(&initial).unwrap();
    let initial_read = initial.read_checked(address, &word(4), domain).unwrap();

    let SlotStateKind::Input { source, bindings } = initial_root.kind() else {
        panic!("partial storage must retain its initial observed source");
    };
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].index, U256::from(4));
    assert_eq!(&bindings[0].numeric, top.numeric());
    assert!(matches!(bindings[0].value.kind(), ExprKind::Fresh(_)));
    assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
    assert_ne!(&bindings[0].value, source.token());
    assert_eq!(
        source.namespace(),
        U256::from_be_slice(address.as_slice()) << 8
    );
    let base = initial.slot_root(Address::ZERO, false).unwrap();
    let SlotStateKind::Input {
        source: base_source,
        bindings: base_bindings,
    } = base.kind()
    else {
        panic!("unobserved accounts must share the fixed world source");
    };
    assert_eq!(source.token(), base_source.token());
    assert_eq!(base_source.namespace(), U256::ZERO);
    assert!(base_bindings.is_empty());
    assert_eq!(
        initial.symbolic_facts(false).collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(&bindings[0].value, &top)])
    );

    assert_eq!(initial_read.numeric(), top.numeric());
    assert_eq!(initial_read.provenance(), top.provenance());
    let initial_expression = initial_read.expression().unwrap();
    let ExprKind::SlotRead { state, key } = initial_expression.kind() else {
        panic!("the initial read must remain bound to its observed source");
    };
    assert_eq!(state, &initial_root);
    assert_eq!(key, &ExprId::constant(U256::from(4)));
    assert_eq!(
        initial_read,
        top.clone().with_expression(initial_expression.clone())
    );

    let mut written = initial.clone();
    written.write(address, &word(4), &top, domain);

    assert_eq!(written.slots(), initial.slots());
    assert_eq!(written.slots().get(&(address, U256::from(4))), Some(&top));
    assert_ne!(
        written, initial,
        "strong writes discard initial dependencies"
    );
    assert!(written.work_size() > initial.work_size());

    let written_root = written.slot_root(address, false).unwrap();
    let SlotStateKind::Store {
        previous,
        key,
        value,
    } = written_root.kind()
    else {
        panic!("the strong write must replace the live read while retaining history");
    };
    assert_eq!(previous, &initial_root);
    assert_eq!(key, &ExprId::constant(U256::from(4)));
    assert!(matches!(value.kind(), ExprKind::Fresh(_)));
    assert_ne!(value, &bindings[0].value);
    let written_read = written.read_checked(address, &word(4), domain).unwrap();
    assert_eq!(written_read.numeric(), initial_read.numeric());
    assert_eq!(written_read.provenance(), initial_read.provenance());
    assert_eq!(written_read.expression(), Some(value));
    assert_eq!(written_read, top.clone().with_expression(value.clone()));
    // These words may have equal concrete values; only their bindings differ.
    assert_ne!(written_read.expression(), initial_read.expression());
    assert_eq!(
        written.symbolic_facts(false).collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(&bindings[0].value, &top), (value, &top)])
    );
    assert_eq!(
        written.read_checked(address, &word(4), domain).unwrap(),
        written_read
    );
    assert_eq!(
        initial.read_checked(address, &word(4), domain).unwrap(),
        initial_read
    );
    assert_eq!(initial_snapshot.state(), &initial);
    assert_eq!(serde_json::to_value(&initial).unwrap(), initial_json);

    let written_json = serde_json::to_value(&written).unwrap();
    assert_ne!(written_json, initial_json);
    let owner = address.to_string();
    let initial_graph = &initial_json["persistent"]["roots"][&owner];
    let written_graph = &written_json["persistent"]["roots"][&owner];
    let initial_nodes = initial_graph["nodes"].as_array().unwrap();
    let written_nodes = written_graph["nodes"].as_array().unwrap();
    let observed = &initial_nodes[initial_graph["root"].as_u64().unwrap() as usize]["SlotsInput"];
    let observed_bindings = observed["bindings"].as_array().unwrap();
    assert_eq!(observed_bindings.len(), 1);
    assert_eq!(
        observed_bindings[0]["index"],
        serde_json::json!(U256::from(4))
    );
    assert_eq!(
        observed_bindings[0]["numeric"],
        serde_json::json!(top.numeric())
    );
    assert_eq!(
        observed["source"]["namespace"],
        serde_json::json!(source.namespace())
    );
    assert_eq!(
        initial_nodes[observed["source"]["token"].as_u64().unwrap() as usize],
        serde_json::to_value(source.token()).unwrap()["nodes"][0]
    );
    assert_eq!(
        initial_nodes[observed_bindings[0]["value"].as_u64().unwrap() as usize],
        serde_json::to_value(&bindings[0].value).unwrap()["nodes"][0]
    );
    assert!(
        initial_graph["identity_scope"]
            .as_str()
            .is_some_and(|scope| !scope.is_empty())
    );
    assert_eq!(
        written_graph["identity_scope"],
        initial_graph["identity_scope"]
    );
    assert_eq!(&written_nodes[..initial_nodes.len()], initial_nodes);
    let write = &written_nodes[written_graph["root"].as_u64().unwrap() as usize]["Store"];
    assert_eq!(write["previous"], initial_graph["root"]);
    assert_eq!(
        written_nodes[write["key"].as_u64().unwrap() as usize],
        serde_json::json!({"Constant": U256::from(4)})
    );
    let ExprKind::Fresh(written_token) = value.kind() else {
        unreachable!("the checked write root contains a fresh word");
    };
    assert_eq!(
        written_nodes[write["value"].as_u64().unwrap() as usize],
        serde_json::json!({"Fresh": written_token})
    );
    // Only this account's root and operand facts change. Compare the entire
    // export so numeric caches, source bindings and every other model field stay checked.
    let mut expected_json = initial_json.clone();
    expected_json["persistent"]["roots"][&owner] = serde_json::to_value(&written_root).unwrap();
    expected_json["persistent"]["operands"] = written
        .symbolic_facts(false)
        .map(|(expression, value)| serde_json::json!({"expression": expression, "value": value}))
        .collect();
    assert_eq!(written_json, expected_json);

    let mut restored = written.clone();
    restored.restore(initial_snapshot);
    assert_eq!(restored, initial);
    assert_eq!(restored.slot_root(address, false).unwrap(), initial_root);
    assert_eq!(
        restored.read_checked(address, &word(4), domain).unwrap(),
        initial_read
    );
    assert_eq!(serde_json::to_value(&restored).unwrap(), initial_json);
}

#[test]
fn a_product_singleton_is_a_strong_storage_key() {
    let (world, address) = partial_world();
    let mut store = Store::new(&world);
    let initial_root = store.slot_root(address, false).unwrap();
    let key = AbstractValue::unsigned_range(U256::from(4), U256::from(4)).unwrap();
    assert!(key.constants().is_none());
    store.write(address, &key, &word(7), Domain::default());
    assert_eq!(store.read(address, &word(4), Domain::default()), word(7));
    assert_eq!(store.read(address, &key, Domain::default()), word(7));
    let root = store.slot_root(address, false).unwrap();
    let SlotStateKind::Store {
        previous,
        key: written_key,
        value,
    } = root.kind()
    else {
        panic!("a numeric product singleton must produce a strong write");
    };
    assert_eq!(previous, &initial_root);
    assert_eq!(written_key, &ExprId::constant(U256::from(4)));
    assert_eq!(value, &ExprId::constant(U256::from(7)));
    assert_eq!(
        store
            .read_checked(address, &key, Domain::default())
            .unwrap(),
        word(7)
    );
}
