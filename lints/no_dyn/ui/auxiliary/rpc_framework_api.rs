// edition:2024
// compile-flags: --cap-lints=allow
// no-prefer-dynamic

#![crate_type = "rlib"]

use std::{future::Future, pin::Pin};

// Model Alloy's default response mapper and erased transport future without
// adding the SDK to the lint tool's independent dependency lockfile.
pub struct RpcCall<Map = fn(u8) -> u8>(Map);

pub fn request() -> RpcCall {
    RpcCall(|value| value)
}

impl<Map: FnOnce(u8) -> u8> RpcCall<Map> {
    pub fn resolve(self) -> u8 {
        (self.0)(1)
    }
}

pub type TransportFut = Pin<Box<dyn Future<Output = u8> + Send>>;

pub fn transport() -> TransportFut {
    Box::pin(async { 1 })
}
