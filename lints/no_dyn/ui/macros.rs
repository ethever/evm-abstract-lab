// edition:2024

#![allow(dead_code)]

fn identity(value: u8) -> u8 {
    value
}

macro_rules! callback {
    () => {{
        let action: fn(u8) -> u8 = identity;
        action
    }};
}

macro_rules! nested {
    () => {
        callback!()
    };
}

std::thread_local! {
    // The type was supplied by local source to a standard library macro.
    static ACTION: fn(u8) -> u8 = identity;
}

fn main() {
    let _ = format!("{:?}", nested!());
    let _ = format!("{:?}", Box::new(1u8) as Box<dyn std::fmt::Debug>);
}
