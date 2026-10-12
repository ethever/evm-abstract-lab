// edition:2024
// compile-flags: --crate-name evm_abstract
// aux-build: framework_api.rs
// aux-build: rpc_framework_api.rs

extern crate framework_api;
extern crate rpc_framework_api;

mod world {
    pub mod rpc {
        pub mod framework {
            use std::{future::Future, pin::Pin};

            pub struct Client;

            impl framework_api::App for Client {
                fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
                    Some(self)
                }
            }

            mod transport {
                pub fn request() -> std::pin::Pin<Box<dyn std::future::Future<Output = u8> + Send>> {
                    Box::pin(async { 1 })
                }
            }

            pub fn run() {
                let mut client = Client;
                assert!(framework_api::App::as_any_mut(&mut client).unwrap().is::<Client>());
                let _pending: Pin<Box<dyn Future<Output = u8> + Send>> = transport::request();
                let identity: fn(u8) -> u8 = |value| value;
                assert_eq!(identity(1), 1);

                // Both types hide dispatch in a dependency-owned signature.
                let call = rpc_framework_api::request();
                assert_eq!(call.resolve(), 1);
                let _pending = rpc_framework_api::transport();
            }
        }
    }
}

fn main() {
    world::rpc::framework::run();
}
