// edition:2024
// compile-flags: --crate-name evm_abstract
// aux-build: framework_api.rs

extern crate framework_api;

mod world {
    pub mod rpc {
        pub mod framework {
            pub struct Client;
        }

        mod loader {
            impl framework_api::App for super::framework::Client {
                fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
                    Some(self)
                }
            }
        }
    }
}

fn main() {}
