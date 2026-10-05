#![allow(dead_code)]

trait Work {
    fn run(&self);
}
impl Work for u8 {
    fn run(&self) {}
}

type Erased = Box<dyn Work>;
struct Nested {
    callback: Option<fn(u8) -> u8>,
    work: Vec<Erased>,
}
fn erased(value: &dyn Work) {
    value.run();
}
fn static_function(value: u8) -> u8 {
    value
}
fn coerced() {
    let value = 1u8;
    erased(&value);
    let callback: fn(u8) -> u8 = static_function;
    let _ = callback(1);
    let boxed: Erased = Box::new(value);
    boxed.run();
}
macro_rules! nested {
    () => {
        fn generated(value: &dyn Work) {
            value.run();
        }
    };
}
nested!();

fn main() {}
