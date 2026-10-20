use analysis_kernel::{
    Capability, Channel, Cons, Context, Here, Meter, Nil, Read, Run, algebra::PublicationPolicy,
};

pub struct Needed;
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

pub fn present<J, M: Meter, P: PublicationPolicy<Value = u32>>(
    run: &mut Run<'_, J, M>,
    provider: Channel<Needed, P>,
) {
    let mut stores = Cons::new(provider, Nil);
    let mut cx = Context::new(run, &mut stores);
    requires(&mut cx);
}
