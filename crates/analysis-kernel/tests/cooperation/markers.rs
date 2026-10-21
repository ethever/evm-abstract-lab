//! Marker contracts for borrowed values and independently owned query families.
use super::support::{Ledger, Maximum, scope};
use analysis_kernel::{Ascending, Capability, Channel, Here, Kernel, Scope, SlotMeta, There};
use std::{cell::Cell, marker::PhantomData, mem, rc::Rc};

struct BorrowedValue<'a>(&'a Cell<usize>);
impl Drop for BorrowedValue<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn metadata<V>(kernel: &mut Kernel<()>, ledger: &mut Ledger, scope: Scope, _: &V) -> SlotMeta<V> {
    kernel.slot(scope, ledger).unwrap()
}

fn shorten_slot<'long: 'short, 'short>(
    slot: SlotMeta<BorrowedValue<'long>>,
    _: &'short (),
) -> SlotMeta<BorrowedValue<'short>> {
    slot
}

#[test]
fn metadata_clones_without_cloning_or_dropping_a_borrowed_value() {
    let mut ledger = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut ledger, 1, 0);
    let drops = Cell::new(0);
    let value = BorrowedValue(&drops);
    {
        let slot = metadata(&mut kernel, &mut ledger, scope, &value);
        let shorter = ();
        let snapshot = shorten_slot(slot.clone(), &shorter);
        assert_eq!(snapshot.scope(), slot.scope());
        assert!(!mem::needs_drop::<SlotMeta<BorrowedValue<'_>>>());
    }
    assert_eq!(drops.get(), 0);
    drop(value);
    assert_eq!(drops.get(), 1);
}

// The query vocabulary is thread-local; its published keys/values are not.
struct LocalQuery<'a>(PhantomData<&'a Rc<()>>);
impl Capability for LocalQuery<'_> {
    type Key = u8;
    type Value = u32;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &u32) -> usize {
        1
    }
}

fn channel<'a>(
    kernel: &mut Kernel<()>,
    ledger: &mut Ledger,
    _: &'a Rc<()>,
) -> Channel<LocalQuery<'a>, Ascending<Maximum>> {
    kernel.channel(Ascending(Maximum), ledger).unwrap()
}

fn assert_send_sync<T: Send + Sync>(_: &T) {}

#[test]
fn channel_auto_traits_follow_stored_data_instead_of_query_vocabulary() {
    let mut kernel = Kernel::new().unwrap();
    let mut ledger = Ledger::default();
    let query = Rc::new(());
    let channel = channel(&mut kernel, &mut ledger, &query);
    assert_send_sync(&channel);
    assert!(channel.is_empty());
}

fn shorten_route<'long: 'short, 'short>(
    route: Option<There<&'long str>>,
    _: &'short (),
) -> Option<There<&'short str>> {
    route
}

#[test]
fn static_routes_remain_zero_sized_and_covariant() {
    let shorter = ();
    let route = shorten_route(None::<There<&'static str>>, &shorter);
    assert!(route.is_none());
    assert_eq!(mem::size_of::<There<There<Here>>>(), 0);
    assert_send_sync(&None::<There<There<Here>>>);
}
