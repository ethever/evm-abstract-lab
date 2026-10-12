// edition:2024
// compile-flags: --crate-name evm_abstract_web
// aux-build: framework_api.rs

extern crate framework_api;

mod framework {
    pub struct BrowserApp;

    pub struct Listeners {
        pub callback: Box<dyn FnMut()>,
    }

    pub fn run() {
        let _app = BrowserApp;
        let _listeners = Listeners {
            callback: Box::new(|| {}),
        };
    }
}

// A framework-owned Self type does not exempt an impl in another module.
mod widgets {
    impl framework_api::App for crate::framework::BrowserApp {
        fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
            Some(self)
        }
    }
}

// A crate-root impl is outside the framework module as well.
impl Drop for framework::Listeners {
    fn drop(&mut self) {
        framework_api::remove_callback(self.callback.as_ref());
    }
}

fn main() {
    framework::run();
}
