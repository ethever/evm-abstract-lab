// edition:2024
// compile-flags: --crate-name evm_abstract_web

mod widgets {
    // Only the crate's top-level framework module is the adapter boundary.
    pub mod framework {
        pub fn run() {
            let callback: fn() = || {};
            callback();
        }
    }
}

mod framework_extra {
    pub fn run() {
        let callback: fn() = || {};
        callback();
    }
}

fn main() {
    widgets::framework::run();
    framework_extra::run();
}
