// edition:2024

mod framework {
    pub fn run() {
        let callback: fn() = || {};
        callback();
    }
}

fn main() {
    framework::run();
}
