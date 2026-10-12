use super::*;
use crate::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, analyze_world},
    bytecode::Program,
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprLimits},
    },
    world::{Account, ByteArray, Code, Entry, World},
};
use serde::ser::SerializeSeq;

#[test]
fn byte_policy_fails_before_allocating_the_unadmitted_write() {
    let mut writer = Writer {
        bytes: Vec::new(),
        limits: JsonLimits { max_bytes: 8 },
        cancellation: Cancellation::default(),
        failure: None,
    };
    writer.write_all(b"1234567").unwrap();
    assert!(writer.write_all(&[0; 4096]).is_err());
    assert_eq!(writer.bytes, b"1234567");
    assert!(matches!(
        writer.failure,
        Some(JsonError::Bytes {
            required: 4103,
            limit: 8,
            written: 7
        })
    ));
    let control = Control::default();
    assert!(
        matches!(with_control(&"a".repeat(4096), JsonLimits { max_bytes: 128 }, &control), Err(JsonError::Bytes { limit: 128, written, .. }) if written <= 128)
    );
}

struct CancelDuring<'a>(&'a Cancellation);
impl Serialize for CancelDuring<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(2))?;
        sequence.serialize_element("retained")?;
        self.0.cancel();
        sequence.serialize_element("must not append")?;
        sequence.end()
    }
}
#[test]
fn cancellation_before_and_during_output_is_typed_and_does_not_poison_other_exports() {
    let control = Control::new(Default::default());
    assert!(
        matches!(with_control(&CancelDuring(control.cancellation()), JsonLimits::default(), &control), Err(JsonError::Cancelled { written }) if written > 0)
    );
    assert!(matches!(
        with_control(&vec![1, 2], JsonLimits::default(), &control),
        Err(JsonError::Cancelled { written: 0 })
    ));
    assert_eq!(compact(&vec![1, 2]).unwrap(), "[1,2]");
}

#[test]
fn real_native_frontier_and_streamed_account_evidence_retain_the_native_shape() {
    let address = Address::repeat_byte(0x11);
    let program = Program::from_hex("366000803700").unwrap();
    let mut world = World::new(Fork::Osaka, "bounded native export");
    let mut account = Account::empty();
    account.code = Code::Runtime(program);
    world.insert(address, account).unwrap();
    let analysis = analyze_world(
        world,
        Entry::concrete(
            address,
            address,
            AbstractValue::constant(U256::ZERO),
            ByteArray::unknown(),
        ),
        ExecutionConfig::default(),
    )
    .unwrap();
    let native = serde_json::to_value(&analysis).unwrap();
    let serialized = pretty(&crate::render::world::evidence(&analysis).unwrap()).unwrap();
    let mut augmented: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert!(augmented["world"].get("fingerprint").is_some());
    assert_eq!(
        augmented["world"]["fingerprint"],
        serde_json::json!(analysis.world().fingerprint().unwrap())
    );
    assert!(
        augmented["world"]["code_hashes"]
            .as_object()
            .unwrap()
            .contains_key(&address.to_string())
    );
    let world = augmented["world"].as_object_mut().unwrap();
    world.remove("fingerprint");
    world.remove("code_hashes");
    for outcome in augmented["outcomes"].as_array_mut().unwrap() {
        assert!(
            outcome["store"]
                .as_object_mut()
                .unwrap()
                .remove("account_observations")
                .is_some()
        );
    }
    assert_eq!(augmented, native);
    assert!(
        native["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| !f["range"].is_null())
    );
    assert!(matches!(
        with_control(
            &analysis,
            JsonLimits { max_bytes: 128 },
            &Control::default()
        ),
        Err(JsonError::Bytes { limit: 128, .. })
    ));
    assert_eq!(
        serde_json::to_value(&analysis).unwrap(),
        native,
        "export failure must not alter analysis facts"
    );
}

fn symbolic_balance_analysis(expression: ExprId) -> crate::analysis::WorldAnalysis {
    let address = Address::repeat_byte(0x22);
    let mut world = World::new(Fork::Osaka, "fingerprint export boundary");
    let mut account = Account::from_hex("00", Fork::Osaka).unwrap();
    account.balance = AbstractValue::top().with_expression(expression);
    world.insert(address, account).unwrap();
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_depth = 512;
    analyze_world(
        world,
        Entry::concrete(
            address,
            address,
            AbstractValue::constant(U256::ZERO),
            ByteArray::exact([]),
        ),
        config,
    )
    .unwrap()
}

#[test]
fn cancelled_exact_fingerprint_is_reported_before_native_evidence_output() {
    let analysis = symbolic_balance_analysis(ExprId::fresh().unwrap());
    let fingerprint = analysis.world().fingerprint().unwrap();
    let control = Control::default();
    control.cancellation().cancel();
    control.scope(|| {
        assert!(matches!(
            crate::render::world::evidence(&analysis),
            Err(crate::world::FingerprintError::Cancelled)
        ));
        assert!(matches!(
            crate::render::world::json(&analysis),
            Err(JsonError::Fingerprint(
                crate::world::FingerprintError::Cancelled
            ))
        ));
        for rendered in [
            crate::render::world::text(&analysis),
            crate::render::world::dot(&analysis),
        ] {
            assert!(rendered.contains("Snapshot fingerprint unavailable:"));
            assert!(rendered.contains("cancelled"));
            assert!(!rendered.contains(&fingerprint.to_string()));
        }
    });
    assert_eq!(analysis.world().fingerprint().unwrap(), fingerprint);
    assert_eq!(
        crate::render::world::json(&analysis).unwrap()["world"]["fingerprint"],
        serde_json::json!(fingerprint)
    );
}

#[test]
fn exact_fingerprint_encoding_boundary_cannot_be_serialized_as_a_digest() {
    let mut expression = ExprId::fresh().unwrap();
    for _ in 0..257 {
        expression = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[expression, ExprId::constant(U256::from(1))],
            ExprLimits {
                max_nodes: 65_536,
                max_depth: 512,
            },
        )
        .unwrap();
    }
    let analysis = symbolic_balance_analysis(expression);
    assert!(matches!(
        crate::render::world::evidence(&analysis),
        Err(crate::world::FingerprintError::Encoding(_))
    ));
    assert!(matches!(
        crate::render::world::json(&analysis),
        Err(JsonError::Fingerprint(
            crate::world::FingerprintError::Encoding(_)
        ))
    ));
    for rendered in [
        crate::render::world::text(&analysis),
        crate::render::world::dot(&analysis),
    ] {
        assert!(rendered.contains("Snapshot fingerprint unavailable:"));
    }
}
