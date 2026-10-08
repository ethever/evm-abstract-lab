// The engine's no_dyn boundary also covers serde's error paths. These visitors
// report concrete errors through Error::custom instead of serde's dyn Expected.
macro_rules! record {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$field_meta:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
        pub struct $name { $($(#[$field_meta])* pub $field: $ty),* }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct RecordVisitor;
                impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(stringify!($name))
                    }
                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                        $(let mut $field: Option<$ty> = None;)*
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::custom(concat!("duplicate field: ", stringify!($field))));
                                    }
                                    $field = Some(map.next_value()?);
                                },)*
                                _ => return Err(serde::de::Error::custom(format!("unknown {} field: {key}", stringify!($name)))),
                            }
                        }
                        Ok($name { $($field: $field.ok_or_else(|| serde::de::Error::custom(concat!("missing field: ", stringify!($field))))?),* })
                    }
                }
                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
}

macro_rules! choice {
    ($(#[$meta:meta])* pub enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
        pub enum $name { $($(#[$variant_meta])* $variant),* }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let name = <String as serde::Deserialize>::deserialize(deserializer)?;
                match name.as_str() {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err(serde::de::Error::custom(format!("unknown {} variant: {name}", stringify!($name)))),
                }
            }
        }
    };
}
