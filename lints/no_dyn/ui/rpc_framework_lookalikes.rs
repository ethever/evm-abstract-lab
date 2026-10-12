// edition:2024
// compile-flags: --crate-name evm_abstract

mod framework {
    pub fn run() {
        let callback: fn() = || {};
        callback();
    }
}

mod world {
    pub mod rpc {
        pub mod framework_extra {
            pub fn run() {
                let callback: fn() = || {};
                callback();
            }
        }

        pub mod loader {
            pub mod framework {
                pub fn run() {
                    let callback: fn() = || {};
                    callback();
                }
            }
        }
    }
}

fn run_fake() {
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
    world::rpc::framework::run();
}

fn main() {
    framework::run();
    world::rpc::framework_extra::run();
    world::rpc::loader::framework::run();
    run_fake();
}
