// edition:2024
// compile-flags: --cap-lints=allow
// no-prefer-dynamic

#![crate_type = "rlib"]

// Model dependency-owned adapter signatures without weakening the consumer lint.
pub trait App {
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any>;
}

pub fn remove_callback(_callback: &dyn FnMut()) {}
