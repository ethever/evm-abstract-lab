#![allow(dead_code, non_snake_case)]

// A real derive never exempts user field types, including aliases and callbacks.
type ExpectedAlias = Box<dyn serde::de::Expected>;

#[derive(serde::Deserialize)]
struct UserFields {
    #[serde(skip_deserializing)]
    expected: Option<Box<dyn serde::de::Expected>>,
    #[serde(skip_deserializing)]
    alias: Option<ExpectedAlias>,
    #[serde(skip_deserializing)]
    callback: Option<fn(u8) -> u8>,
}

fn handwritten<E: serde::de::Error>() -> E {
    E::invalid_length(1, &"two elements")
}

macro_rules! Deserialize {
    () => {
        fn local_macro<E: serde::de::Error>() -> E {
            E::invalid_length(1, &"two elements")
        }
    };
}
Deserialize!();

fn signature(expected: &dyn serde::de::Expected) {
    let _ = expected;
}

#[derive(serde::Deserialize)]
struct Custom {
    #[serde(deserialize_with = "custom")]
    value: u8,
}

fn custom<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let callback: fn(u8) -> u8 = |value| value;
    let _ = callback;
    serde::Deserialize::deserialize(deserializer)
}

// The Serde coercion exception does not cover standard error interoperability.
#[derive(Debug)]
struct Error;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error")
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

fn main() {}
