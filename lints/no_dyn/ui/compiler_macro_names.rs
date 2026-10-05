// edition:2024

#![allow(dead_code, non_snake_case)]

// Matching only an expansion's name must not exempt user-written macros.
macro_rules! Debug {
    () => {
        fn local_debug(value: &dyn std::fmt::Debug) {
            println!("{value:?}");
        }
    };
}

macro_rules! test {
    () => {
        fn local_test() -> fn(u8) -> u8 {
            panic!("user macro, not the compiler test harness")
        }
    };
}

Debug!();
test!();

// A compiler derive exemption must not cover fields written by the user.
#[derive(Debug)]
struct UserField {
    value: Box<dyn std::fmt::Debug>,
}

fn main() {}
