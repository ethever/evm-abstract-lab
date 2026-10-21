//! Find a semantic phase boundary without depending on the executor's tariff.

/// The caller first proves `reached(funded)` on its straight-line fixture.
/// Only the work allowance varies; all policies and semantic inputs stay fixed.
pub fn first_work_reaching(funded: usize, mut reached: impl FnMut(usize) -> bool) -> usize {
    let (mut minimum, mut maximum) = (1, funded);
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        if reached(midpoint) {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    minimum
}
