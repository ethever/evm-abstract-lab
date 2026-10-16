//! Query-local keys: pay integer navigation and only the structural comparisons
//! that share a fingerprint. A fingerprint is never an equality proof.

use super::{LocalResult, Work};
use crate::domain::symbolic::ExprId;

pub(super) struct KeyMap<T> {
    buckets: Vec<Bucket<T>>,
}

struct Bucket<T> {
    fingerprint: u64,
    entries: Vec<(ExprId, T)>,
}

impl<T> Default for KeyMap<T> {
    fn default() -> Self {
        Self {
            buckets: Vec::new(),
        }
    }
}

impl<T> KeyMap<T> {
    fn locate(&self, fingerprint: u64, work: &mut Work<'_>) -> LocalResult<(usize, bool)> {
        let (mut lower, mut upper) = (0, self.buckets.len());
        while lower < upper {
            work.charge(1)?;
            let middle = lower + (upper - lower) / 2;
            match self.buckets[middle].fingerprint.cmp(&fingerprint) {
                std::cmp::Ordering::Less => lower = middle + 1,
                std::cmp::Ordering::Greater => upper = middle,
                std::cmp::Ordering::Equal => return Ok((middle, true)),
            }
        }
        Ok((lower, false))
    }

    pub(super) fn get<'a>(
        &'a self,
        key: &ExprId,
        work: &mut Work<'_>,
    ) -> LocalResult<Option<&'a T>> {
        let (index, present) = self.locate(key.cached_fingerprint(), work)?;
        if present {
            for (candidate, value) in &self.buckets[index].entries {
                if work.same_expression(key, candidate)? {
                    return Ok(Some(value));
                }
            }
        }
        Ok(None)
    }

    pub(super) fn insert(&mut self, key: ExprId, value: T, work: &mut Work<'_>) -> LocalResult<()> {
        let fingerprint = key.cached_fingerprint();
        let (index, present) = self.locate(fingerprint, work)?;
        if present {
            for (candidate, previous) in &mut self.buckets[index].entries {
                if work.same_expression(&key, candidate)? {
                    work.charge(1)?;
                    *previous = value;
                    return Ok(());
                }
            }
            let entries = &mut self.buckets[index].entries;
            let growth = if entries.len() == entries.capacity() {
                entries.len()
            } else {
                0
            };
            work.charge(growth.saturating_add(1))?;
            entries.push((key, value));
        } else {
            // Vec insertion moves only bucket headers, never expression trees.
            let growth = if self.buckets.len() == self.buckets.capacity() {
                self.buckets.len()
            } else {
                0
            };
            work.charge(
                self.buckets
                    .len()
                    .saturating_sub(index)
                    .saturating_add(growth)
                    .saturating_add(1),
            )?;
            self.buckets.insert(
                index,
                Bucket {
                    fingerprint,
                    entries: vec![(key, value)],
                },
            );
        }
        Ok(())
    }

    pub(super) fn remove(&mut self, key: &ExprId, work: &mut Work<'_>) -> LocalResult<()> {
        let (index, present) = self.locate(key.cached_fingerprint(), work)?;
        if present {
            let entries = &mut self.buckets[index].entries;
            for entry in 0..entries.len() {
                if work.same_expression(key, &entries[entry].0)? {
                    work.charge(1)?;
                    entries.swap_remove(entry);
                    break;
                }
            }
        }
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        self.buckets.clear();
    }
}
