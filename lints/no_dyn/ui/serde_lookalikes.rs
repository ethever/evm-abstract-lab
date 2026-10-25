// edition:2024
// aux-build: serde_lookalike.rs

#![allow(dead_code)]
extern crate serde_lookalike;

// An external proc macro with the same derive name is not the trusted definition.
#[derive(serde_lookalike::Deserialize)]
struct Record;

mod serde_core {
    pub mod de {
        pub trait Expected {}
        impl Expected for &str {}
        pub struct Error;
        impl Error {
            pub fn invalid_length(_: usize, _: &dyn Expected) {}
        }
    }
}

// Same spelled trait and method paths cannot make a handwritten coercion exempt.
fn main() {
    serde_core::de::Error::invalid_length(1, &"two elements");
}
