//! Lightweight transport-safe immutable decompilation schemas.
mod artifact;
mod model;
pub use artifact::*;
pub use model::*;
pub use alloy_primitives::U256;
pub use evm_bytecode::Fork;

fn deserialize_fork<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Fork, D::Error> {
    use serde::Deserialize;
    String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
}
mod decimal_id {
    pub fn serialize<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok,S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de,D:serde::Deserializer<'de>>(deserializer:D)->Result<u64,D::Error>{
        use serde::Deserialize;
        String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
    }
}
