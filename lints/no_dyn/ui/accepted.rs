// edition:2024

use std::fmt;

trait Work {
    fn run(&self) -> u8;
}
impl Work for u8 {
    fn run(&self) -> u8 {
        *self
    }
}
fn generic<T: Work>(value: &T) -> u8 {
    value.run()
}
fn opaque(value: &impl Work) -> u8 {
    value.run()
}
fn callback(action: impl FnOnce(u8) -> u8) -> u8 {
    action(1)
}
fn identity(value: u8) -> u8 {
    value
}

#[derive(Debug)]
enum ApplicationError {
    InvalidInput,
}

#[derive(Debug)]
struct Record {
    value: u8,
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid input")
    }
}

impl std::error::Error for ApplicationError {}

macro_rules! concrete_callback {
    ($value:expr) => {{
        let captured = $value;
        move |input: u8| input + captured
    }};
}

fn main() {
    let action = identity;
    let value = generic(&1u8) + opaque(&2u8) + callback(|x| x + 1) + action(3);
    println!("value={value:?}");
    assert_eq!(value, 8);

    let captured = 4;
    let action = concrete_callback!(captured);
    let boxed = Box::new(action);
    assert_eq!(boxed(1), 5);

    let async_action = async |input: u8| input + captured;
    let _future = async_action(1);
    let _async_block = async move { captured + 1 };

    let error = ApplicationError::InvalidInput;
    println!("{error}: {error:?}");
    let record = Record { value };
    println!("{record:?}");
    assert_eq!(record.value, 8);
}
