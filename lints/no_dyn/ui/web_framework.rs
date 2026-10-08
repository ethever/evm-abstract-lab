// edition:2024
// compile-flags: --crate-name evm_abstract_web

mod framework {
    pub fn invoke(action: Box<dyn Fn()>) {
        action();
    }

    pub fn run() {
        invoke(Box::new(|| {}));
    }
}

fn main() {
    // The coercion itself belongs inside the adapter too.
    framework::run();
}
