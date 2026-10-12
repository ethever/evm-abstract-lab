// edition:2024
// compile-flags: --crate-name evm_abstract
// aux-build: rpc_framework_api.rs

extern crate rpc_framework_api;

mod world {
    pub mod rpc {
        pub mod framework {
            pub fn request() -> rpc_framework_api::RpcCall {
                rpc_framework_api::request()
            }

            pub fn transport() -> rpc_framework_api::TransportFut {
                rpc_framework_api::transport()
            }
        }

        pub mod loader {
            use super::framework;

            pub fn run() {
                // Calling the exempt adapter does not exempt its consumer.
                let call = framework::request();
                let _ = call.resolve();
                let _pending = framework::transport();
            }
        }
    }
}

fn main() {
    world::rpc::loader::run();
}
