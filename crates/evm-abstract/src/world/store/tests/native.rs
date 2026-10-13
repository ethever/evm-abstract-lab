//! Native account planes retain matching symbolic fact entries and failures.

use super::super::*;
use crate::{Fork, domain::relational::RelationLimits, world::Account};

#[test]
fn native_slot_facts_keep_both_planes_and_shared_key_bindings() {
    let address = Address::repeat_byte(0x31);
    let mut world = World::new(Fork::Osaka, "native slot facts");
    world.insert(address, Account::empty()).unwrap();
    let mut store = Store::new(&world);
    let key = AbstractValue::unsigned_range(U256::ONE, U256::from(8))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let domain = Domain::default();
    store
        .write_checked(
            address,
            &key,
            &AbstractValue::constant(U256::from(7)),
            domain,
        )
        .unwrap();
    store
        .write_transient_checked(
            address,
            &key,
            &AbstractValue::constant(U256::from(9)),
            domain,
        )
        .unwrap();
    let json = serde_json::to_value(&store).unwrap();
    for (name, transient) in [("persistent", false), ("transient", true)] {
        let entries = json[name]["operands"].as_array().unwrap();
        assert_eq!(entries.len(), store.symbolic_facts(transient).count());
        for (expression, fact) in store.symbolic_facts(transient) {
            let graph = serde_json::to_value(expression).unwrap();
            let entry = entries.iter().find(|e| e["expression"] == graph).unwrap();
            assert_eq!(entry["value"], serde_json::to_value(fact).unwrap());
        }
        assert!(json[name]["failure"].is_null());
    }
    assert_eq!(
        json["persistent"]["operands"][0]["expression"],
        json["transient"]["operands"][0]["expression"]
    );
}

#[test]
fn native_legacy_slot_failure_cannot_look_like_a_successful_zero_root() {
    let world = World::new(Fork::Osaka, "native failure");
    let mut store = Store::new(&world);
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 1,
        max_depth: 1,
        ..RelationLimits::default()
    }));
    store.write(
        Address::repeat_byte(0x31),
        &AbstractValue::top(),
        &AbstractValue::constant(U256::from(7)),
        domain,
    );
    assert!(store.symbolic_limit_reached());
    let json = serde_json::to_value(&store).unwrap();
    assert_eq!(
        json["persistent"]["failure"].as_str(),
        Some(
            store
                .persistent
                .failure
                .as_ref()
                .unwrap()
                .to_string()
                .as_str()
        )
    );
    assert!(json["transient"]["failure"].is_null());
}

#[test]
fn native_slot_and_log_arrays_keep_shape_order_and_cancel_without_buffering() {
    #[derive(Serialize)]
    struct Arrays<'a> {
        #[serde(serialize_with = "serialize_slots")]
        slots: &'a OrderedMap<(Address, U256), AbstractValue>,
        #[serde(serialize_with = "serialize_logs")]
        logs: &'a OrderedMap<LogKey, AbstractLog>,
    }
    let address = Address::repeat_byte(0x51);
    let slots: OrderedMap<_, _> = [
        (
            (address, U256::from(9)),
            AbstractValue::constant(U256::from(19)),
        ),
        (
            (address, U256::from(3)),
            AbstractValue::constant(U256::from(13)),
        ),
    ]
    .into_iter()
    .collect();
    let logs: OrderedMap<_, _> = [9usize, 3]
        .into_iter()
        .map(|pc| {
            (
                LogKey {
                    address,
                    code_address: address,
                    pc,
                },
                AbstractLog {
                    topics: vec![AbstractValue::constant(U256::from(pc))],
                    data: ByteArray::exact(&[pc as u8]),
                },
            )
        })
        .collect();
    let arrays = Arrays {
        slots: &slots,
        logs: &logs,
    };
    let json = serde_json::to_value(&arrays).unwrap();
    assert_eq!(
        json["slots"][0]["slot"],
        serde_json::to_value(U256::from(3)).unwrap()
    );
    assert_eq!(
        json["slots"][1]["slot"],
        serde_json::to_value(U256::from(9)).unwrap()
    );
    assert_eq!(
        json["slots"][0]["value"],
        serde_json::to_value(AbstractValue::constant(U256::from(13))).unwrap()
    );
    assert_eq!(json["logs"][0]["pc"], 3);
    assert_eq!(json["logs"][1]["pc"], 9);
    assert!(json["logs"][0].get("address").is_some());
    assert!(json["logs"][0].get("topics").is_some());
    assert!(json["logs"][0].get("data").is_some());
    let cancellation = embedded_smt::Cancellation::new();
    cancellation.cancel();
    embedded_smt::with_cancellation(&cancellation, || {
        let error = serde_json::to_vec(&arrays).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    });
}
