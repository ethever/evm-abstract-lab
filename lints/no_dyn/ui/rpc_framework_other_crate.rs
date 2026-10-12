// edition:2024
// compile-flags: --crate-name evm_abstract_server

mod world {
    pub mod rpc {
        pub mod framework {
            pub fn run() {
                let callback: fn() = || {};
                callback();
            }
        }
    }
}

fn main() {
    world::rpc::framework::run();
}
