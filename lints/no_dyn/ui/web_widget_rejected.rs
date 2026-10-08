// edition:2024
// compile-flags: --crate-name evm_abstract_web

mod widgets {
    pub fn run() {
        let callback: fn() = || {};
        callback();
    }
}

fn main() {
    widgets::run();
}
