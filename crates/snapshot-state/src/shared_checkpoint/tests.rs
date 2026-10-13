//! Shared captures must avoid deep clones without aliasing restored owners.

use super::SharedCheckpoint;
use crate::{Checkpoint, OrderedMap};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct TrackedState {
    values: Vec<u64>,
    clones: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl Clone for TrackedState {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            values: self.values.clone(),
            clones: Arc::clone(&self.clones),
            drops: Arc::clone(&self.drops),
        }
    }
}

impl Drop for TrackedState {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn tracked(values: &[u64]) -> (TrackedState, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let clones = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    (
        TrackedState {
            values: values.to_vec(),
            clones: Arc::clone(&clones),
            drops: Arc::clone(&drops),
        },
        clones,
        drops,
    )
}

#[test]
fn capture_clones_once_and_checkpoint_clones_share_until_the_last_drop() {
    let (mut current, clones, drops) = tracked(&[1, 2, 3]);
    let saved = SharedCheckpoint::capture(&current);
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    let siblings: Vec<_> = (0..64).map(|_| saved.clone()).collect();
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    for sibling in &siblings {
        assert!(std::ptr::eq(saved.state(), sibling.state()));
    }
    current.values[0] = 99;
    assert_eq!(saved.state().values, vec![1, 2, 3]);
    drop(saved);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(siblings);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(current.values, vec![99, 2, 3]);
    drop(current);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn from_state_and_unique_restore_move_without_cloning() {
    let (owned, clones, drops) = tracked(&[1, 2]);
    let saved = SharedCheckpoint::from_state(owned);
    let mut current = TrackedState {
        values: vec![99],
        clones: Arc::clone(&clones),
        drops: Arc::clone(&drops),
    };
    assert_eq!(clones.load(Ordering::SeqCst), 0);
    saved.restore(&mut current);
    assert_eq!(current.values, vec![1, 2]);
    assert_eq!(clones.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(current);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn shared_restore_clones_once_and_keeps_the_remaining_version_immutable() {
    let (owned, clones, drops) = tracked(&[1, 2]);
    let saved = SharedCheckpoint::from_state(owned);
    let sibling = saved.clone();
    let mut current = TrackedState {
        values: vec![99],
        clones: Arc::clone(&clones),
        drops: Arc::clone(&drops),
    };
    saved.restore(&mut current);
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(!std::ptr::eq(&current, sibling.state()));
    current.values[0] = 42;
    assert_eq!(sibling.state().values, vec![1, 2]);
    // The final handle can now restore by moving its captured state.
    sibling.restore(&mut current);
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert_eq!(current.values, vec![1, 2]);
    drop(current);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
fn branches_and_outer_rollback_keep_independent_complete_versions() {
    let mut current = OrderedMap::from_iter([(1, vec![10]), (2, vec![20])]);
    let root = SharedCheckpoint::capture(&current);
    current.get_mut(&1).unwrap().push(11);
    let left = SharedCheckpoint::capture(&current);
    root.clone().restore(&mut current);
    current.get_mut(&2).unwrap().push(21);
    let right = SharedCheckpoint::capture(&current);
    let successful_child = SharedCheckpoint::capture(&current);
    current.get_mut(&1).unwrap().push(12);
    drop(successful_child);
    assert_eq!(current[&1], vec![10, 12]);
    left.clone().restore(&mut current);
    assert_eq!(current[&1], vec![10, 11]);
    assert_eq!(current[&2], vec![20]);
    right.restore(&mut current);
    assert_eq!(current[&1], vec![10]);
    assert_eq!(current[&2], vec![20, 21]);
    root.restore(&mut current);
    assert_eq!(current[&1], vec![10]);
    assert_eq!(current[&2], vec![20]);
    assert_eq!(left.state()[&1], vec![10, 11]);
}

#[test]
fn value_equality_and_serialization_and_debug_match_owned_checkpoints() {
    let values = OrderedMap::from_iter([(2, vec![20, 21]), (1, vec![10])]);
    let saved = SharedCheckpoint::from_state(values.clone());
    let independent = SharedCheckpoint::from_state(values.clone());
    assert!(!std::ptr::eq(saved.state(), independent.state()));
    assert_eq!(saved, independent);
    assert_eq!(saved, saved.clone());
    assert_ne!(
        saved,
        SharedCheckpoint::from_state(OrderedMap::from_iter([(1, vec![11])]))
    );
    let owned = Checkpoint::from_state(values.clone());
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(&owned).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(&values).unwrap()
    );
    assert_eq!(format!("{saved:?}"), format!("{owned:?}"));
    assert_eq!(format!("{saved:#?}"), format!("{owned:#?}"));
}

#[test]
fn original_owned_checkpoint_restore_still_accepts_non_clone_states() {
    struct NonClone(Vec<u8>);
    let saved = Checkpoint::from_state(NonClone(vec![1, 2, 3]));
    let mut current = NonClone(vec![9]);
    saved.restore(&mut current);
    assert_eq!(current.0, vec![1, 2, 3]);
}
