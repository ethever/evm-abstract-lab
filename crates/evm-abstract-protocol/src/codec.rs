// Records require an object and every key without an explicit omission default,
// including nullable fields. Defaults never apply to malformed present values.
// Plain Serde derives also accept sequences and default missing Option fields.
macro_rules! record {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$field_meta:meta])* pub $field:ident: $ty:ty $(= $default:expr)?),* $(,)? }) => {
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
                        Ok($name { $($field: match $field {
                            Some(value) => value,
                            None => record!(@missing $field $(, $default)?)?,
                        }),* })
                    }
                }
                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
    (@missing $field:ident) => {
        Err(serde::de::Error::custom(concat!("missing field: ", stringify!($field))))
    };
    (@missing $field:ident, $default:expr) => {
        Ok($default)
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

// Standard externally tagged enums retain string unit variants and exactly one
// payload key. Records and string-only choices above keep their stricter shapes.
macro_rules! variant {
    ($(#[$meta:meta])* pub enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident $(($ty:ty))?),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name { $($(#[$variant_meta])* $variant $(($ty))?),* }
    };
}
