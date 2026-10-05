// edition:2024

fn first(value: u8) -> u8 {
    value
}

fn second(value: u8) -> u8 {
    value + 1
}

fn main() {
    // Different function items are implicitly reified into function pointers.
    let actions = [first, second];
    let _ = actions[0](1);

    // Different noncapturing closures can also be coerced without a fn type.
    let action = if true {
        |value: u8| value
    } else {
        |value: u8| value + 1
    };
    let _ = action(1);
}
