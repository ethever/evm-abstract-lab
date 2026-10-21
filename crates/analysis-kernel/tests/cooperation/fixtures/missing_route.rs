use analysis_kernel::{Capability, Context, Here, Meter, Nil, Read, Run};

struct Needed;
impl Capability for Needed {
    type Key = ();
    type Value = u32;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &u32) -> usize {
        1
    }
}
fn requires<Cx: Read<Needed, Here>>(_: &mut Cx) {}

pub fn missing<J, M: Meter>(run: &mut Run<'_, J, M>) {
    let mut stores = Nil;
    let mut cx = Context::new(run, &mut stores);
    requires(&mut cx);
}
