#![allow(dead_code)]

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    value: u8,
    optional: Option<String>,
}

#[derive(serde::Deserialize)]
enum Payload {
    Unit,
    Value(Record),
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Tagged {
    Offline { label: String },
    Chain { number: u64 },
}

// Renaming a dependency or derive import must retain the same definition identity.
use serde as wire;
use wire::Deserialize as Decode;

#[derive(Decode)]
struct Renamed {
    data: Vec<Payload>,
}

macro_rules! record {
    () => {
        #[derive(wire::Deserialize)]
        struct NestedDerive {
            value: u64,
        }
    };
}
record!();

fn main() {}
