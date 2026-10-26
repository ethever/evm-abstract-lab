//! Scoped controls observe real native fanout construction without changing it.

#[derive(Clone, Copy, Debug)]
pub(super) struct Counts {
    pub enabled: bool,
    pub forms: usize,
    pub copies: usize,
}

pub(super) static CONTROL: crate::test_instrumentation::Counter<Counts> =
    crate::test_instrumentation::Counter::new(Counts {
        enabled: true,
        forms: 0,
        copies: 0,
    });

pub(super) fn enabled() -> bool {
    let mut enabled = true;
    CONTROL.update(|control| enabled = control.enabled);
    enabled
}

pub(super) fn formed() {
    CONTROL.update(|control| control.forms += 1);
}

pub(super) fn copied() {
    CONTROL.update(|control| control.copies += 1);
}
