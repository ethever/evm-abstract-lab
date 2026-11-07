//! Oversized wire prefixes must stop before later records are deserialized.
use super::*;

fn payload() -> LiftedErrorPayload {
    let message = "x".repeat(super::super::MAX_ERROR_PAYLOAD_BYTES - 68);
    let mut bytes = vec![0; 68 + message.len()];
    bytes[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    bytes[35] = 32;
    bytes[36..68].copy_from_slice(&U256::from(message.len()).to_be_bytes::<32>());
    bytes[68..].copy_from_slice(message.as_bytes());
    LiftedErrorPayload {
        id: EntityId(1),
        message,
        bytes,
    }
}

fn revert() -> LiftedRevert {
    LiftedRevert {
        id: EntityId(2),
        block: EntityId(3),
        operation: EntityId(4),
        memory: EntityId(5),
        offset: EntityId(6),
        size: EntityId(7),
        payload: EntityId(1),
        writes: vec![EntityId(8); super::super::MAX_ERROR_PAYLOAD_BYTES],
    }
}

fn stops_before_later_record<T: serde::Serialize + WireWeight>(field: &str, record: &T) {
    let count = MAX_LIFTED_RECORDS / record.wire_weight() + 1;
    let encoded = serde_json::to_string(record).unwrap();
    let prefix = std::iter::repeat_n(encoded.as_str(), count)
        .collect::<Vec<_>>()
        .join(",");
    // Every prefix record is individually bounded. If all were collected before
    // charging their weight, the invalid boolean record would be decoded first.
    let wire = format!(
        "{{\"expressions\":[],\"blocks\":[],\"transfers\":[],\"controls\":[],\"{field}\":[{prefix},false]}}"
    );
    let error = serde_json::from_str::<LiftedProgram>(&wire).unwrap_err();
    assert!(
        error.to_string().contains("weighted semantic arena limit"),
        "{field}: {error}"
    );
}

#[test]
fn variable_size_payload_and_write_arenas_refuse_oversized_prefixes_early() {
    stops_before_later_record("payloads", &payload());
    stops_before_later_record("reverts", &revert());
}

#[test]
fn new_arena_weights_match_the_global_record_units() {
    let payload = payload();
    let revert = revert();
    let construction = LiftedConstruction {
        id: EntityId(9),
        block: EntityId(3),
        start: 0,
        end: 1,
        revert: EntityId(2),
    };
    let weight = payload.wire_weight() + revert.wire_weight() + construction.wire_weight();
    let program = LiftedProgram {
        payloads: vec![payload],
        reverts: vec![revert],
        constructions: vec![construction],
        ..LiftedProgram::default()
    };
    assert_eq!(program.record_count(), weight);
}

#[test]
fn sharing_wire_weights_cover_body_references_and_identity_invocations() {
    let procedure = crate::SharedProcedure {
        id: EntityId(1),
        body: EntityId(100),
        interface: crate::SharedInterface::default(),
    };
    let calls: Vec<_> = (0_u64..2)
        .map(|i| crate::SharedInvocation {
            id: EntityId(i + 2),
            procedure: procedure.id,
            source: EntityId(100 + i),
            owner: None,
            position: 0,
            continuation: None,
            bindings: Vec::new(),
        })
        .collect();
    let expected =
        procedure.wire_weight() + calls.iter().map(WireWeight::wire_weight).sum::<usize>();
    let program = LiftedProgram {
        procedures: vec![procedure],
        invocations: calls,
        ..LiftedProgram::default()
    };
    assert_eq!(program.record_count(), expected);
    program.validate().unwrap();
    let wire = serde_json::to_vec(&program).unwrap();
    let admitted: LiftedProgram = serde_json::from_slice(&wire).unwrap();
    assert_eq!(admitted.record_count(), expected);
}
