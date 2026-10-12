// edition:2024
// compile-flags: --crate-name evm_abstract_web
// aux-build: framework_api.rs

extern crate framework_api;

mod framework {
    pub mod browser {
        use framework_api::App;

        pub struct BrowserApp;

        impl App for BrowserApp {
            fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
                Some(self)
            }
        }

        mod shortcut {
            pub struct Listeners {
                keydown: Box<dyn FnMut()>,
                composition_start: Box<dyn FnMut()>,
                composition_end: Box<dyn FnMut()>,
                blur: Box<dyn FnMut()>,
            }

            impl Drop for Listeners {
                fn drop(&mut self) {
                    framework_api::remove_callback(self.keydown.as_ref());
                    framework_api::remove_callback(self.composition_start.as_ref());
                    framework_api::remove_callback(self.composition_end.as_ref());
                    framework_api::remove_callback(self.blur.as_ref());
                }
            }

            pub fn install() -> Listeners {
                Listeners {
                    keydown: Box::new(|| {}),
                    composition_start: Box::new(|| {}),
                    composition_end: Box::new(|| {}),
                    blur: Box::new(|| {}),
                }
            }
        }

        pub fn run() {
            let mut app = BrowserApp;
            assert!(app.as_any_mut().unwrap().is::<BrowserApp>());
            let _listeners = shortcut::install();
        }
    }
}

fn main() {
    framework::browser::run();
}
