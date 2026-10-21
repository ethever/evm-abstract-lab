use super::JoinWork;
use crate::domain::{NumericValue, WordElement, WordObservation};
use alloy_primitives::U256;
use serde::Serialize;
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Serialize)]
struct LargeWord {
    numeric: NumericValue,
    #[serde(skip)]
    inline: [u64; 1024],
    #[serde(skip)]
    heap: Vec<u64>,
    #[serde(skip)]
    clones: Option<Arc<AtomicUsize>>,
}

impl PartialEq for LargeWord {
    fn eq(&self, other: &Self) -> bool {
        self.numeric == other.numeric && self.inline == other.inline && self.heap == other.heap
    }
}
impl Eq for LargeWord {}

impl Clone for LargeWord {
    fn clone(&self) -> Self {
        if let Some(count) = &self.clones {
            count.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            numeric: self.numeric.clone(),
            inline: self.inline,
            heap: self.heap.clone(),
            clones: self.clones.as_ref().map(Arc::clone),
        }
    }
}
impl fmt::Display for LargeWord {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.numeric, out)
    }
}
impl WordElement for LargeWord {
    fn top() -> Self {
        Self::from_observation(&WordObservation::top())
    }
    fn constant(value: U256) -> Self {
        Self::from_observation(&WordObservation::constant(value))
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(value),
            inline: [0; 1024],
            heap: Vec::new(),
            clones: None,
        }
    }
    fn observation(&self) -> WordObservation {
        self.numeric.observation()
    }
    fn is_top(&self) -> bool {
        self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        self.numeric.work_size() + self.inline.len() + self.heap.len()
    }
}

#[test]
fn quoting_covers_deep_custom_release_and_large_inline_transport_without_cloning() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut word = LargeWord::top();
    word.clones = Some(Arc::clone(&clones));
    let small = JoinWork::quote::<LargeWord>(word.work_size(), 1, 1, 1, word.work_size(), 0);
    word.heap = vec![0; 8192];
    let large = JoinWork::quote::<LargeWord>(word.work_size(), 1, 1, 1, word.work_size(), 0);
    assert_eq!(large.installation - small.installation, 2 * 8192);
    let inline = JoinWork::quote::<LargeWord>(1, 1, 1, 1, 1, 0);
    let compact = JoinWork::quote::<NumericValue>(1, 1, 1, 1, 1, 0);
    assert!(inline.installation >= compact.installation + 1024);
    assert!(inline.preparation >= compact.preparation + 1024);
    assert_eq!(clones.load(Ordering::Relaxed), 0);
}
