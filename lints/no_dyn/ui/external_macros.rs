// edition:2024
// aux-build: dispatch_types.rs

extern crate dispatch_types;

fn identity(value: u8) -> u8 {
    value
}

fn main() {
    let _ = dispatch_types::erase!(1u8);
    let _ = dispatch_types::callback!(identity);
    let _ = dispatch_types::forward!(identity as fn(u8) -> u8);
}
