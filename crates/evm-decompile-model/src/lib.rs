//! Lightweight transport-safe immutable decompilation schemas.
/// Current immutable artifact wire schema.
pub const SCHEMA_VERSION: u32 = 1;

// Declare each record once, retaining explicit null separately from absence.
// Enum payload adapters use these same object-only field/duplicate checks.
macro_rules! strict_record {
    ($(#[$meta:meta])* pub struct $name:ident $(<$parameter:ident = $default:ty>)? { $($(#[$field_meta:meta])* pub $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(serde::Serialize)]
        pub struct $name $(<$parameter = $default>)? { $($(#[$field_meta])* pub $field:$ty),* }
        impl<'de $(, $parameter)?> serde::Deserialize<'de> for $name $(<$parameter>)?
        where Self: Sized, $($parameter: serde::Deserialize<'de>,)? {
            fn deserialize<D:serde::Deserializer<'de>>(deserializer:D)->Result<Self,D::Error> {
                struct Visitor<T>(std::marker::PhantomData<T>);
                impl<'de $(, $parameter)?> serde::de::Visitor<'de> for Visitor<$name $(<$parameter>)?>
                where $name $(<$parameter>)?: Sized, $($parameter: serde::Deserialize<'de>,)? {
                    type Value=$name $(<$parameter>)?;
                    fn expecting(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str(concat!(stringify!($name)," object"))}
                    fn visit_map<A:serde::de::MapAccess<'de>>(self,mut map:A)->Result<Self::Value,A::Error> {
                        $(let mut $field: Option<$ty> = None;)*
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {return Err(serde::de::Error::duplicate_field(stringify!($field)));}
                                    $field = Some(map.next_value()?);
                                },)*
                                _ => return Err(serde::de::Error::unknown_field(&key, &[$(stringify!($field)),*])),
                            }
                        }
                        Ok($name { $($field:$field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?),* })
                    }
                }
                deserializer.deserialize_map(Visitor::<Self>(std::marker::PhantomData))
            }
        }
    }
}

mod artifact;
mod codec;
mod model;
mod structured;
pub use alloy_primitives::U256;
pub use artifact::*;
pub use evm_bytecode::Fork;
pub use model::*;
pub use structured::{MAX_STRUCTURED_WIRE_DEPTH, MAX_STRUCTURED_WIRE_NODES, MAX_STRUCTURING_DEPTH};

mod decimal_id {
    pub fn serialize<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        use serde::Deserialize;
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
