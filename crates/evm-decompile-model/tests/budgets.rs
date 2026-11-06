//! Lossless numeric resource policy, independently of the native executor width.

use evm_decompile_model::{
    Artifact, Limits, MAX_STRUCTURING_DEPTH, Phase, Progress, SCHEMA_VERSION, Statistics,
};
use serde::{Serialize, de::DeserializeOwned};

const LIMIT_FIELDS: [&str; 10] = [
    "max_bytecode_bytes",
    "max_blocks",
    "max_operations",
    "max_values",
    "max_work",
    "max_iterations",
    "max_constant_set",
    "max_cloned_blocks",
    "max_structuring_depth",
    "max_history_entities",
];
const STATISTIC_FIELDS: [&str; 8] = [
    "work",
    "evaluations",
    "blocks",
    "operations",
    "values",
    "edges",
    "cloned_blocks",
    "history_entities",
];

fn numeric_record<T>(record: &T, fields: &[&str], count: u64)
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let raw = serde_json::to_string(record).unwrap();
    let restored: T = serde_json::from_str(&raw).unwrap();
    assert_eq!(&restored, record);
    let object: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for field in fields {
        assert_eq!(object[field].as_u64(), Some(count), "{field}: {raw}");
        let token = format!("\"{field}\":{count}");
        assert!(raw.contains(&token), "expected a numeric token: {raw}");
        // Use raw tokens: Value would already round an overflowing JSON integer
        // to f64 before the record's integer decoder could reject it.
        for invalid in [
            "18446744073709551616",
            "-1",
            "0.5",
            "1.0",
            "1e0",
            "\"18446744073709551615\"",
            "null",
        ] {
            let malformed = raw.replacen(&token, &format!("\"{field}\":{invalid}"), 1);
            assert!(
                serde_json::from_str::<T>(&malformed).is_err(),
                "accepted {field}={invalid}: {malformed}"
            );
        }
        let duplicate = format!("{{\"{field}\":{count},{}", &raw[1..]);
        assert!(serde_json::from_str::<T>(&duplicate).is_err(), "{field}");
        let mut missing = object.clone();
        missing.as_object_mut().unwrap().remove(*field);
        assert!(serde_json::from_value::<T>(missing).is_err(), "{field}");
    }
}

#[test]
fn defaults_maximize_resource_ceilings_and_preserve_precision_and_hard_depth() {
    let limits = Limits::default();
    for resource in [
        limits.max_bytecode_bytes,
        limits.max_blocks,
        limits.max_operations,
        limits.max_values,
        limits.max_work,
        limits.max_iterations,
        limits.max_cloned_blocks,
        limits.max_history_entities,
    ] {
        assert_eq!(resource, u64::MAX);
    }
    assert_eq!(limits.max_constant_set, 8_u64);
    assert_eq!(limits.max_structuring_depth, 512_u64);
    assert_eq!(MAX_STRUCTURING_DEPTH, 512);
    assert_eq!(SCHEMA_VERSION, 4);
}

#[test]
fn every_limit_and_counter_is_a_strict_lossless_u64_json_number() {
    for count in [
        0,
        u64::from(u32::MAX),
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        // Depth and precision can be decoded independently of native execution
        // validation; the shared wire model must transport every u64 exactly.
        let limits = Limits {
            max_bytecode_bytes: count,
            max_blocks: count,
            max_operations: count,
            max_values: count,
            max_work: count,
            max_iterations: count,
            max_constant_set: count,
            max_cloned_blocks: count,
            max_structuring_depth: count,
            max_history_entities: count,
        };
        numeric_record(&limits, &LIMIT_FIELDS, count);
        let progress = Progress {
            phase: Phase::Recovery,
            blocks: count,
            operations: count,
            work: count,
            stage: None,
        };
        numeric_record(&progress, &["blocks", "operations", "work"], count);
        let statistics = Statistics {
            work: count,
            evaluations: count,
            blocks: count,
            operations: count,
            values: count,
            edges: count,
            cloned_blocks: count,
            history_entities: count,
        };
        numeric_record(&statistics, &STATISTIC_FIELDS, count);
    }
}

#[test]
fn unchanged_legacy_artifact_keeps_its_numeric_policy_and_schema() {
    let raw = include_str!("fixtures/partial-artifact.json");
    let artifact: Artifact = serde_json::from_str(raw).unwrap();
    assert_eq!(artifact.schema_version, 1);
    let expected = Limits {
        max_bytecode_bytes: 100_000,
        max_blocks: 8_192,
        max_operations: 100_000,
        max_values: 500_000,
        max_work: 100_000_000,
        max_iterations: 100_000,
        max_constant_set: 8,
        max_cloned_blocks: 2_048,
        max_structuring_depth: 128,
        max_history_entities: 2_000_000,
    };
    assert_eq!(artifact.config.limits, expected);
    assert_eq!(artifact.statistics.work, 30_174);
    assert_eq!(artifact.statistics.history_entities, 3_584);
    let before: serde_json::Value = serde_json::from_str(raw).unwrap();
    let after = serde_json::to_value(&artifact).unwrap();
    assert_eq!(after["config"], before["config"]);
    assert_eq!(after["statistics"], before["statistics"]);
    assert_eq!(after["schema_version"], before["schema_version"]);
    for field in LIMIT_FIELDS {
        assert!(after["config"]["limits"][field].is_u64(), "{field}");
    }
}
