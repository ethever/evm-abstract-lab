//! Snapshot input uses Serde's strict derives; storage additionally rejects duplicate slot keys.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) fork: String,
    pub(super) provenance: String,
    pub(super) identity: Option<Identity>,
    pub(super) fingerprint: Option<String>,
    pub(super) accounts: Vec<SnapshotAccount>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum Identity {
    Offline {
        label: String,
    },
    Chain {
        chain_id: String,
        block_hash: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotAccount {
    pub(super) address: String,
    pub(super) code: Option<String>,
    #[serde(default, deserialize_with = "deserialize_storage")]
    pub(super) storage: BTreeMap<String, String>,
    #[serde(default = "unknown_storage")]
    pub(super) storage_unknown: bool,
    pub(super) balance: Option<String>,
    pub(super) nonce: Option<String>,
    pub(super) existence: Option<String>,
    pub(super) code_hash: Option<String>,
}

fn unknown_storage() -> bool {
    true
}

fn deserialize_storage<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Slots;
    impl<'de> serde::de::Visitor<'de> for Slots {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a storage object with unique slot keys")
        }
        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut slots = BTreeMap::new();
            while let Some((slot, value)) = map.next_entry::<String, String>()? {
                if slots.insert(slot.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate storage key {slot}"
                    )));
                }
            }
            Ok(slots)
        }
    }
    deserializer.deserialize_map(Slots)
}
