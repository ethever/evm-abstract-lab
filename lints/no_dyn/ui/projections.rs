// edition:2024
// aux-build: dispatch_types.rs

#![allow(dead_code)]

extern crate dispatch_types;

use dispatch_types::{Associated, CallbackOwner, ErasedOwner};

struct Holder {
    action: <CallbackOwner as Associated>::Value,
    object: <ErasedOwner as Associated>::Value,
}

// A diverging body contains no erased expression; the signature must be checked.
fn callback() -> <CallbackOwner as Associated>::Value {
    panic!("signature only")
}

fn borrowed<'a>() -> dispatch_types::Borrowed<'a> {
    panic!("late-bound lifetime in erased return type")
}

trait Required {
    fn callback() -> <CallbackOwner as Associated>::Value;

    fn object() -> <ErasedOwner as Associated>::Value {
        panic!("trait default signature")
    }
}

fn main() {}
