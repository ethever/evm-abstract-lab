use std::rc::Rc;

use super::{KeyIndex, PolicyKey, checkpoint};
use crate::resource::WorkBudget;

mod cancellation;
mod carrier;
mod index;
mod pricing;

use carrier::{HeavyKey, Probe};

fn seeded(probe: &Rc<Probe>) -> KeyIndex<HeavyKey> {
    let mut index = KeyIndex::new();
    let mut budget = WorkBudget::new(usize::MAX);
    index
        .insert_with(HeavyKey::new(vec![1; 4096], probe), &mut budget, |_| Ok(17))
        .unwrap();
    probe.reset();
    index
}

fn snapshot(index: &KeyIndex<HeavyKey>) -> Vec<(Vec<u8>, usize)> {
    index
        .iter()
        .map(|(key, value)| (key.bytes().to_vec(), *value))
        .collect()
}

fn caches(index: &KeyIndex<HeavyKey>) -> (usize, usize, usize, usize) {
    (
        index.len(),
        index.maximum_key_work,
        index.total_key_work,
        index.work_size(),
    )
}
