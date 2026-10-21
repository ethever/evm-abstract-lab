use std::rc::Rc;

use super::{HeavyKey, KeyIndex, PolicyKey, Probe, caches, checkpoint, seeded, snapshot};
use crate::{analysis::machine::MachineError, resource::WorkBudget};

#[test]
fn unfunded_or_refused_keys_never_compare_or_build_native_state() {
    for work in [None, Some(usize::MAX)] {
        let probe = Rc::new(Probe::default());
        let mut index = seeded(&probe);
        let before = snapshot(&index);
        let cache = caches(&index);
        let mut native = Vec::new();
        let mut key = HeavyKey::new(vec![1; 4096], &probe);
        if let Some(work) = work {
            key = key.with_work(work);
        }
        let available = if work.is_some() { usize::MAX } else { 64 };
        let mut lookup_budget = WorkBudget::new(available);
        assert_eq!(index.get(&key, &mut lookup_budget), Err(MachineError::Work));
        assert_eq!(lookup_budget.used(), 0);
        let mut insertion_budget = WorkBudget::new(available);
        assert_eq!(
            index.insert_with(key, &mut insertion_budget, |_| {
                native.push(99);
                Ok(99)
            }),
            Err(MachineError::Work)
        );
        assert!(native.is_empty());
        assert_eq!(probe.quotes.get(), 2);
        assert_eq!(probe.clones.get(), 0);
        assert_eq!(probe.comparisons.get(), 0);
        assert_eq!(insertion_budget.used(), 0);
        assert_eq!(snapshot(&index), before);
        assert_eq!(caches(&index), cache);
    }
    let mut empty = KeyIndex::new();
    assert!(empty.is_empty());
    let probe = Rc::new(Probe::default());
    let mut budget = WorkBudget::new(usize::MAX);
    let key = HeavyKey::new(vec![7; 4096], &probe).with_work(usize::MAX);
    assert_eq!(empty.get(&key, &mut budget), Err(MachineError::Work));
    assert_eq!(
        empty.insert_with(key, &mut budget, |_| panic!("MAX builder")),
        Err(MachineError::Work)
    );
    assert!(empty.is_empty());
    assert_eq!(probe.comparisons.get(), 0);
}

#[test]
fn search_and_cache_overflow_refuse_before_comparison_or_builder() {
    let probe = Rc::new(Probe::default());
    let mut index = KeyIndex::new();
    let mut budget = WorkBudget::new(usize::MAX);
    let huge = usize::MAX / 2 + 100;
    index
        .insert_with(
            HeavyKey::new(vec![1], &probe).with_work(huge),
            &mut budget,
            |_| Ok(17),
        )
        .unwrap();
    let before = snapshot(&index);
    let cache = caches(&index);
    assert!(index.work_size() < usize::MAX);
    probe.reset();
    let incoming = HeavyKey::new(vec![2], &probe).with_work(huge);
    assert_eq!(index.get(&incoming, &mut budget), Err(MachineError::Work));
    assert_eq!(
        index.insert_with(incoming, &mut budget, |_| panic!("overflow builder")),
        Err(MachineError::Work)
    );
    assert_eq!(probe.comparisons.get(), 0);
    assert_eq!(probe.clones.get(), 0);
    assert_eq!(snapshot(&index), before);
    assert_eq!(caches(&index), cache);

    let mut empty = KeyIndex::new();
    assert_eq!(
        empty.insert_with(
            HeavyKey::new(vec![3], &probe).with_work(usize::MAX - 1),
            &mut budget,
            |_| panic!("retention overflow builder"),
        ),
        Err(MachineError::Work)
    );
    assert!(empty.is_empty());
    assert_eq!(empty.total_key_work, 0);
}

#[test]
fn failed_builder_and_occupied_entry_keep_the_complete_index_unchanged() {
    let probe = Rc::new(Probe::default());
    let mut index = seeded(&probe);
    let before = snapshot(&index);
    let cache = caches(&index);
    let mut budget = WorkBudget::new(usize::MAX);
    let mut built = false;
    let mut native = vec![17];
    assert_eq!(
        index.insert_with(
            HeavyKey::new(vec![2; 4096], &probe),
            &mut budget,
            |budget| {
                built = true;
                assert!(probe.comparisons.get() > 0);
                assert!(budget.charge(7));
                let mut staged = native.clone();
                staged.push(23);
                assert_eq!(staged, vec![17, 23]);
                Err(MachineError::States)
            }
        ),
        Err(MachineError::States)
    );
    assert!(built);
    assert_eq!(native, vec![17]);
    assert!(budget.used() > 7);
    assert_eq!(snapshot(&index), before);
    assert_eq!(caches(&index), cache);
    assert_eq!(
        index.insert_with(HeavyKey::new(vec![1; 4096], &probe), &mut budget, |_| {
            native.push(99);
            Ok(99)
        }),
        Err(MachineError::Shape)
    );
    assert_eq!(native, vec![17]);
    assert_eq!(snapshot(&index), before);
    assert_eq!(caches(&index), cache);
    assert_eq!(probe.clones.get(), 0);
}

#[test]
fn vacant_entry_commit_never_compares_after_successful_native_builder() {
    let probe = Rc::new(Probe::default());
    let mut index = seeded(&probe);
    let cache = caches(&index);
    let key = HeavyKey::new(vec![2; 8192], &probe);
    let key_work = key.key_work();
    probe.reset();
    let mut native = vec![17];
    let mut budget = WorkBudget::new(usize::MAX);
    let value = index
        .insert_with(key, &mut budget, |budget| {
            assert!(probe.comparisons.get() > 0);
            checkpoint(budget)?;
            native.push(23);
            probe.forbid_comparisons.set(true);
            Ok(native[1])
        })
        .unwrap();
    probe.forbid_comparisons.set(false);
    assert_eq!(value, 23);
    assert_eq!(native, vec![17, 23]);
    assert_eq!(index.len(), 2);
    assert_eq!(index.total_key_work, cache.2 + key_work);
    assert_eq!(index.maximum_key_work, key_work.max(cache.1));
    assert_eq!(probe.quotes.get(), 1);
    assert_eq!(probe.clones.get(), 0);
    assert_eq!(index.values().copied().collect::<Vec<_>>(), native);

    let same_contents = HeavyKey::new(vec![2; 8192], &probe).with_work(key_work * 2);
    assert_eq!(index.get(&same_contents, &mut budget), Ok(Some(23)));
    let mut different_contents = vec![2; 8192];
    different_contents[8191] = 3;
    assert_eq!(
        index.get(&HeavyKey::new(different_contents, &probe), &mut budget),
        Ok(None)
    );
    assert!(probe.compared_bytes.get() >= 8192);
    assert_eq!(index.total_key_work, cache.2 + key_work);
}

#[test]
fn cached_total_prices_a_real_full_copy_without_quoting_or_comparing_keys() {
    let probe = Rc::new(Probe::default());
    let mut index = seeded(&probe);
    let mut insertion_budget = WorkBudget::new(usize::MAX);
    index
        .insert_with(
            HeavyKey::new(vec![2; 8192], &probe),
            &mut insertion_budget,
            |_| Ok(23),
        )
        .unwrap();
    let before = snapshot(&index);
    let cache = caches(&index);
    probe.reset();
    let work = index.work_size();
    assert_eq!(probe.quotes.get(), 0);
    assert!(work > index.total_key_work);
    let mut budget = WorkBudget::new(work);
    assert!(budget.charge(work));
    let copy = index.clone();
    checkpoint(&budget).unwrap();
    assert_eq!(probe.quotes.get(), 0);
    assert_eq!(probe.comparisons.get(), 0);
    assert_eq!(probe.clones.get(), 2);
    assert_eq!(probe.cloned_bytes.get(), 4096 + 8192);
    assert_eq!(snapshot(&copy), before);
    assert_eq!(caches(&copy), cache);
    assert_eq!(budget.used(), work);
}

#[test]
fn btree_height_quotes_cover_real_comparisons_and_avoid_quadratic_admission_fees() {
    fn measure(length: usize) -> (usize, usize) {
        let probe = Rc::new(Probe::default());
        let mut index = KeyIndex::new();
        let mut budget = WorkBudget::new(usize::MAX);
        let mut native = Vec::new();
        for value in 0..length {
            let key = HeavyKey::new((value as u64).to_be_bytes().to_vec(), &probe);
            let key_work = key.key_work();
            probe.reset();
            let comparisons = index.comparison_limit();
            let paid_before = budget.used();
            assert_eq!(
                index.insert_with(key, &mut budget, |_| {
                    native.push(value);
                    probe.forbid_comparisons.set(true);
                    Ok(value)
                }),
                Ok(value)
            );
            probe.forbid_comparisons.set(false);
            assert!(probe.comparisons.get() <= comparisons);
            assert!(budget.used() - paid_before >= probe.comparisons.get() * key_work * 2);
            assert_eq!(probe.clones.get(), 0);
        }
        assert_eq!(index.values().copied().collect::<Vec<_>>(), native);
        let insertion_work = budget.used();
        let mut query_budget = WorkBudget::new(usize::MAX);
        for value in 0..=length {
            let key = HeavyKey::new((value as u64).to_be_bytes().to_vec(), &probe);
            let key_work = key.key_work();
            probe.reset();
            let paid_before = query_budget.used();
            let expected = (value < length).then_some(value);
            assert_eq!(index.get(&key, &mut query_budget), Ok(expected));
            assert!(probe.comparisons.get() <= index.comparison_limit());
            assert!(query_budget.used() - paid_before >= probe.comparisons.get() * key_work * 2);
            assert_eq!(probe.quotes.get(), 1);
            assert_eq!(probe.clones.get(), 0);
        }
        (insertion_work, query_budget.used())
    }

    let small = measure(64);
    let large = measure(512);
    // Eight times as many admissions/lookups may grow by a logarithmic factor;
    // an all-key tariff would grow approximately 64-fold here.
    assert!(large.0 > small.0);
    assert!(large.0 < small.0 * 24);
    assert!(large.1 > small.1);
    assert!(large.1 < small.1 * 24);
    assert!(KeyIndex::<HeavyKey>::levels(usize::MAX) <= usize::BITS as usize);
}
