//! Concrete JSON visitors preserve field presence, duplicate keys and tagged identity order.

use serde::{
    Deserialize, Deserializer,
    de::{Error as _, MapAccess, SeqAccess, Visitor},
};
use std::{collections::BTreeMap, fmt};

pub(super) struct Snapshot {
    pub(super) fork: String,
    pub(super) provenance: String,
    pub(super) identity: Option<Identity>,
    pub(super) fingerprint: Option<String>,
    pub(super) accounts: Vec<SnapshotAccount>,
}

pub(super) enum Identity {
    Offline {
        label: String,
    },
    Chain {
        chain_id: String,
        block_hash: String,
    },
}

pub(super) struct SnapshotAccount {
    pub(super) address: String,
    pub(super) code: Option<String>,
    pub(super) storage: BTreeMap<String, String>,
    pub(super) storage_unknown: bool,
    pub(super) balance: Option<String>,
    pub(super) nonce: Option<String>,
    pub(super) existence: Option<String>,
    pub(super) code_hash: Option<String>,
}

// Keep the visitors concrete: serde's derive-generated identifier/length errors
// coerce expected values to trait objects, even for these JSON-only input types.
fn read_field<'de, M, T>(
    map: &mut M,
    field: &mut Option<T>,
    name: &'static str,
) -> Result<(), M::Error>
where
    M: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if field.is_some() {
        return Err(M::Error::duplicate_field(name));
    }
    *field = Some(map.next_value()?);
    Ok(())
}

fn required<T, E: serde::de::Error>(field: Option<T>, name: &'static str) -> Result<T, E> {
    field.ok_or_else(|| E::missing_field(name))
}

fn sequence_field<'de, S, T>(
    sequence: &mut S,
    index: usize,
    expected: &'static str,
) -> Result<T, S::Error>
where
    S: SeqAccess<'de>,
    T: Deserialize<'de>,
{
    sequence.next_element()?.ok_or_else(|| {
        S::Error::custom(format_args!("invalid length {index}, expected {expected}"))
    })
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["fork", "provenance", "identity", "fingerprint", "accounts"];
        struct SnapshotVisitor;
        impl<'de> Visitor<'de> for SnapshotVisitor {
            type Value = Snapshot;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("struct Snapshot")
            }

            fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Snapshot, S::Error> {
                const EXPECTED: &str = "struct Snapshot with 5 elements";
                Ok(Snapshot {
                    fork: sequence_field(&mut sequence, 0, EXPECTED)?,
                    provenance: sequence_field(&mut sequence, 1, EXPECTED)?,
                    identity: sequence_field(&mut sequence, 2, EXPECTED)?,
                    fingerprint: sequence_field(&mut sequence, 3, EXPECTED)?,
                    accounts: sequence_field(&mut sequence, 4, EXPECTED)?,
                })
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Snapshot, M::Error> {
                let (mut fork, mut provenance, mut identity, mut fingerprint, mut accounts) =
                    (None, None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "fork" => read_field(&mut map, &mut fork, "fork")?,
                        "provenance" => read_field(&mut map, &mut provenance, "provenance")?,
                        "identity" => read_field(&mut map, &mut identity, "identity")?,
                        "fingerprint" => read_field(&mut map, &mut fingerprint, "fingerprint")?,
                        "accounts" => read_field(&mut map, &mut accounts, "accounts")?,
                        _ => return Err(M::Error::unknown_field(&field, FIELDS)),
                    }
                }
                Ok(Snapshot {
                    fork: required(fork, "fork")?,
                    provenance: required(provenance, "provenance")?,
                    // The outer Option tracks presence, including a supplied null.
                    identity: identity.unwrap_or(None),
                    fingerprint: fingerprint.unwrap_or(None),
                    accounts: required(accounts, "accounts")?,
                })
            }
        }
        deserializer.deserialize_struct("Snapshot", FIELDS, SnapshotVisitor)
    }
}

enum IdentityKind {
    Offline,
    Chain,
}

impl<'de> Deserialize<'de> for IdentityKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KindVisitor;
        impl Visitor<'_> for KindVisitor {
            type Value = IdentityKind;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("variant identifier")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<IdentityKind, E> {
                match value {
                    "offline" => Ok(IdentityKind::Offline),
                    "chain" => Ok(IdentityKind::Chain),
                    _ => Err(E::unknown_variant(value, &["offline", "chain"])),
                }
            }
        }
        deserializer.deserialize_identifier(KindVisitor)
    }
}

fn identity_field<E: serde::de::Error>(
    field: &mut Option<String>,
    name: &'static str,
    value: serde_json::Value,
) -> Result<(), E> {
    if field.is_some() {
        return Err(E::duplicate_field(name));
    }
    *field = Some(String::deserialize(value).map_err(E::custom)?);
    Ok(())
}

impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdentityVisitor;
        impl<'de> Visitor<'de> for IdentityVisitor {
            type Value = Identity;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("internally tagged enum Identity")
            }

            fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Identity, S::Error> {
                let kind = required(sequence.next_element()?, "kind")?;
                match kind {
                    IdentityKind::Offline => Ok(Identity::Offline {
                        label: sequence_field(
                            &mut sequence,
                            0,
                            "struct variant Identity::Offline with 1 element",
                        )?,
                    }),
                    IdentityKind::Chain => Ok(Identity::Chain {
                        chain_id: sequence_field(
                            &mut sequence,
                            0,
                            "struct variant Identity::Chain with 2 elements",
                        )?,
                        block_hash: sequence_field(
                            &mut sequence,
                            1,
                            "struct variant Identity::Chain with 2 elements",
                        )?,
                    }),
                }
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Identity, M::Error> {
                let mut kind = None;
                let mut fields = Vec::new();
                while let Some(field) = map.next_key::<String>()? {
                    if field == "kind" {
                        read_field(&mut map, &mut kind, "kind")?;
                    } else {
                        // Buffer in source order so the tag can appear anywhere,
                        // and retain duplicate keys for the selected variant.
                        fields.push((field, map.next_value::<serde_json::Value>()?));
                    }
                }
                match required(kind, "kind")? {
                    IdentityKind::Offline => {
                        let mut label = None;
                        for (field, value) in fields {
                            match field.as_str() {
                                "label" => identity_field::<M::Error>(&mut label, "label", value)?,
                                _ => return Err(M::Error::unknown_field(&field, &["label"])),
                            }
                        }
                        Ok(Identity::Offline {
                            label: required(label, "label")?,
                        })
                    }
                    IdentityKind::Chain => {
                        let (mut chain_id, mut block_hash) = (None, None);
                        for (field, value) in fields {
                            match field.as_str() {
                                "chain_id" => {
                                    identity_field::<M::Error>(&mut chain_id, "chain_id", value)?
                                }
                                "block_hash" => identity_field::<M::Error>(
                                    &mut block_hash,
                                    "block_hash",
                                    value,
                                )?,
                                _ => {
                                    return Err(M::Error::unknown_field(
                                        &field,
                                        &["chain_id", "block_hash"],
                                    ));
                                }
                            }
                        }
                        Ok(Identity::Chain {
                            chain_id: required(chain_id, "chain_id")?,
                            block_hash: required(block_hash, "block_hash")?,
                        })
                    }
                }
            }
        }
        deserializer.deserialize_any(IdentityVisitor)
    }
}

struct Storage(BTreeMap<String, String>);

impl<'de> Deserialize<'de> for Storage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_storage(deserializer).map(Self)
    }
}

impl<'de> Deserialize<'de> for SnapshotAccount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &[
            "address",
            "code",
            "storage",
            "storage_unknown",
            "balance",
            "nonce",
            "existence",
            "code_hash",
        ];
        struct AccountVisitor;
        impl<'de> Visitor<'de> for AccountVisitor {
            type Value = SnapshotAccount;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("struct SnapshotAccount")
            }

            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut sequence: S,
            ) -> Result<SnapshotAccount, S::Error> {
                const EXPECTED: &str = "struct SnapshotAccount with 8 elements";
                Ok(SnapshotAccount {
                    address: sequence_field(&mut sequence, 0, EXPECTED)?,
                    code: sequence_field(&mut sequence, 1, EXPECTED)?,
                    storage: sequence
                        .next_element::<Storage>()?
                        .map(|storage| storage.0)
                        .unwrap_or_default(),
                    storage_unknown: sequence.next_element()?.unwrap_or_else(unknown_storage),
                    balance: sequence_field(&mut sequence, 4, EXPECTED)?,
                    nonce: sequence_field(&mut sequence, 5, EXPECTED)?,
                    existence: sequence_field(&mut sequence, 6, EXPECTED)?,
                    code_hash: sequence_field(&mut sequence, 7, EXPECTED)?,
                })
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<SnapshotAccount, M::Error> {
                let (mut address, mut code, mut storage, mut storage_unknown) =
                    (None, None, None, None);
                let (mut balance, mut nonce, mut existence, mut code_hash) =
                    (None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "address" => read_field(&mut map, &mut address, "address")?,
                        "code" => read_field(&mut map, &mut code, "code")?,
                        "storage" => read_field::<_, Storage>(&mut map, &mut storage, "storage")?,
                        "storage_unknown" => {
                            read_field(&mut map, &mut storage_unknown, "storage_unknown")?
                        }
                        "balance" => read_field(&mut map, &mut balance, "balance")?,
                        "nonce" => read_field(&mut map, &mut nonce, "nonce")?,
                        "existence" => read_field(&mut map, &mut existence, "existence")?,
                        "code_hash" => read_field(&mut map, &mut code_hash, "code_hash")?,
                        _ => return Err(M::Error::unknown_field(&field, FIELDS)),
                    }
                }
                Ok(SnapshotAccount {
                    address: required(address, "address")?,
                    code: code.unwrap_or(None),
                    storage: storage.map(|storage| storage.0).unwrap_or_default(),
                    storage_unknown: storage_unknown.unwrap_or_else(unknown_storage),
                    balance: balance.unwrap_or(None),
                    nonce: nonce.unwrap_or(None),
                    existence: existence.unwrap_or(None),
                    code_hash: code_hash.unwrap_or(None),
                })
            }
        }
        deserializer.deserialize_struct("SnapshotAccount", FIELDS, AccountVisitor)
    }
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
