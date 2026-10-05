// edition:2024
// compile-flags: --cap-lints=allow
// no-prefer-dynamic

#![crate_type = "rlib"]

// This auxiliary crate models a dependency: its implementation is not subject
// to the consumer workspace's no_dyn policy, but its exposed types still are.
pub trait Work {
    fn run(&self) -> u8;
}

impl Work for u8 {
    fn run(&self) -> u8 {
        *self
    }
}

pub type Callback = fn(u8) -> u8;
pub type Erased = Box<dyn Work>;
pub type Borrowed<'a> = &'a dyn Work;

pub fn callback() -> Callback {
    |value| value
}

pub fn erased() -> Erased {
    Box::new(1u8)
}

pub fn borrowed(value: &u8) -> Borrowed<'_> {
    value
}

pub fn accept(value: Borrowed<'_>) -> u8 {
    value.run()
}

pub struct Wrapper(u8);

impl Wrapper {
    pub fn new(value: u8) -> Self {
        Self(value)
    }
}

impl std::ops::Deref for Wrapper {
    type Target = dyn Work;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub trait Associated {
    type Value;
}

pub struct CallbackOwner;
pub struct ErasedOwner;

impl Associated for CallbackOwner {
    type Value = Callback;
}

impl Associated for ErasedOwner {
    type Value = Erased;
}

#[macro_export]
macro_rules! erase {
    ($value:expr) => {
        Box::new($value) as $crate::Erased
    };
}

#[macro_export]
macro_rules! callback {
    ($action:expr) => {{
        let action: $crate::Callback = $action;
        action
    }};
}

#[macro_export]
macro_rules! forward {
    ($value:expr) => {
        $value
    };
}
