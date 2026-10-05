// edition:2024
// aux-build: dispatch_types.rs

#![allow(dead_code)]

extern crate dispatch_types;

struct Holder {
    callbacks: Vec<dispatch_types::Callback>,
    objects: Option<dispatch_types::Erased>,
}

fn callback() -> dispatch_types::Callback {
    panic!("external alias in function return")
}

impl Holder {
    fn object() -> dispatch_types::Erased {
        panic!("external alias in inherent method return")
    }
}

fn main() {}
