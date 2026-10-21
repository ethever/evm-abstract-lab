//! Heap operations are real; counters and cancellation only observe them.

use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    rc::Rc,
};

use analysis_control::Cancellation;

use super::super::{Arrival, PartitionPolicy, PolicyKey, inline_work};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hook {
    Quote,
    Clone,
    Compare,
    Classify,
}

#[derive(Debug, Default)]
pub(super) struct Probe {
    pub quotes: Cell<usize>,
    pub clones: Cell<usize>,
    pub comparisons: Cell<usize>,
    pub classifications: Cell<usize>,
    pub cloned_bytes: Cell<usize>,
    pub compared_bytes: Cell<usize>,
    pub forbid_comparisons: Cell<bool>,
    cancellation: RefCell<Option<(Hook, Cancellation)>>,
}

impl Probe {
    pub fn arm(&self, hook: Hook, cancellation: Cancellation) {
        *self.cancellation.borrow_mut() = Some((hook, cancellation));
    }

    pub fn reset(&self) {
        self.quotes.set(0);
        self.clones.set(0);
        self.comparisons.set(0);
        self.classifications.set(0);
        self.cloned_bytes.set(0);
        self.compared_bytes.set(0);
    }

    fn visit(&self, hook: Hook) {
        let counter = match hook {
            Hook::Quote => &self.quotes,
            Hook::Clone => &self.clones,
            Hook::Compare => {
                assert!(!self.forbid_comparisons.get(), "Ord after native commit");
                &self.comparisons
            }
            Hook::Classify => &self.classifications,
        };
        counter.set(counter.get() + 1);
        if let Some((target, cancellation)) = self.cancellation.borrow().as_ref()
            && *target == hook
        {
            cancellation.cancel();
        }
    }
}

#[derive(Debug)]
pub(super) struct HeavyKey {
    bytes: Vec<u8>,
    work: usize,
    probe: Rc<Probe>,
}

impl HeavyKey {
    pub fn new(bytes: Vec<u8>, probe: &Rc<Probe>) -> Self {
        let work = bytes
            .len()
            .saturating_mul(2)
            .saturating_add(inline_work::<Self>())
            .saturating_add(2);
        Self {
            bytes,
            work,
            probe: Rc::clone(probe),
        }
    }

    pub fn with_work(mut self, work: usize) -> Self {
        self.work = work;
        self
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Clone for HeavyKey {
    fn clone(&self) -> Self {
        self.probe.visit(Hook::Clone);
        self.probe
            .cloned_bytes
            .set(self.probe.cloned_bytes.get() + self.bytes.len());
        Self {
            bytes: self.bytes.clone(),
            work: self.work,
            probe: Rc::clone(&self.probe),
        }
    }
}

impl PartialEq for HeavyKey {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for HeavyKey {}

impl PartialOrd for HeavyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeavyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.probe.visit(Hook::Compare);
        for (left, right) in self.bytes.iter().zip(&other.bytes) {
            self.probe
                .compared_bytes
                .set(self.probe.compared_bytes.get() + 1);
            match left.cmp(right) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        self.bytes.len().cmp(&other.bytes.len())
    }
}

impl PolicyKey for HeavyKey {
    fn key_work(&self) -> usize {
        self.probe.visit(Hook::Quote);
        self.work
    }
}

pub(super) struct Classifier {
    pub work: usize,
    pub probe: Rc<Probe>,
}

impl PartitionPolicy for Classifier {
    type Class = bool;

    fn classify(&self, arrival: Arrival) -> Self::Class {
        self.probe.visit(Hook::Classify);
        arrival.resume_status.unwrap_or(false)
    }

    fn classification_work(&self, _: Arrival) -> usize {
        self.probe.visit(Hook::Quote);
        self.work
    }
}
