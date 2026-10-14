//! Explicit analysis-owned mathematical answers for complete canonical queries.
//! Keys own only canonical bytes. Full byte equality is required; original DAGs,
//! names, pointer identities and native models are never retained. Bounds are an
//! optional optimization: every omission falls back to the unchanged oracle.

mod budget;
mod index;
mod key;
pub use budget::CacheQuery;
#[cfg(test)]
mod tests;

use crate::{Bool, Bv, Outcome, Provider, UniqueOutcome, Unknown};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

/// Caller encoding-policy namespace, checked by the caller before this layer.
/// These neutral scalar bounds isolate successful encodings without importing
/// an application configuration or changing native-query validation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheScope {
    /// Whether the caller's relation encoding is enabled.
    pub enabled: bool,
    /// The caller's retained-conjunction bound.
    pub constraints: usize,
    /// The caller's encoding work bound.
    pub nodes: usize,
    /// The caller's expression nesting bound.
    pub depth: usize,
}
#[derive(Clone, Copy, Debug)]
struct Limits {
    entries: usize,
    retained_bytes: usize,
    key_bytes: usize,
    nodes: usize,
    edges: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 8192,
            retained_bytes: 32 * 1024 * 1024,
            key_bytes: 256 * 1024,
            nodes: 16_384,
            edges: 65_536,
        }
    }
}
// Entries own one canonical allocation; the age index stores only its digest.
// This conservative charge covers the Arc header, bounded bucket allocation,
// answer/timestamp, both BTree metadata records and slack;
// it is a retention policy, not a claim about allocator RSS.
const ENTRY_METADATA: usize = 1024;

/// Bounded counters contain no source/native objects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CacheStats {
    /// Full-key unique-projection hits.
    pub unique_hits: u64,
    /// Full-key candidate SAT/UNSAT hits.
    pub candidate_hits: u64,
    /// Keys successfully constructed but absent.
    pub misses: u64,
    /// Oversize/invalid optional keys that used the oracle.
    pub bypasses: u64,
    /// Results left unrecorded because they were not decided.
    pub undecided: u64,
    /// Cache entries installed.
    pub insertions: u64,
    /// Oldest entries discarded by retention policy.
    pub evictions: u64,
    /// Number of retained mathematical answers.
    pub entries: usize,
    /// Actual bytes in owned canonical key slices and unique literal strings.
    pub payload_bytes: usize,
    /// Payload plus the conservative per-entry metadata charge.
    pub retained_bytes: usize,
    /// Additional logical work paid from the owner's ledger, without refunds.
    pub maintenance_work: u64,
}
#[derive(Clone, Debug)]
enum Answer {
    Unique(UniqueOutcome),
    Candidate(bool),
}
impl Answer {
    fn bytes(&self) -> usize {
        match self {
            Self::Unique(UniqueOutcome::Unique(value)) => value.len(),
            _ => 0,
        }
    }
}
#[derive(Debug)]
struct Entry {
    key: Arc<[u8]>,
    answer: Answer,
    age: u64,
    charge: usize,
    payload: usize,
}
enum Lookup {
    Hit(Answer),
    Miss,
    Bypass,
}
/// Lazy, explicit per-analysis LRU cache; no global state, TLS or native handles.
#[derive(Debug, Default)]
pub struct DecidedCache {
    entries: BTreeMap<index::Digest, Vec<Entry>>,
    ages: BTreeMap<u64, index::Digest>,
    clock: u64,
    limits: Limits,
    stats: CacheStats,
    #[cfg(test)]
    force_collisions: bool,
}
impl DecidedCache {
    /// A bounded observational copy; it does not extend cache lifetime.
    pub fn stats(&self) -> CacheStats {
        self.stats
    }
    fn tick(&mut self) -> Option<u64> {
        self.clock = self.clock.checked_add(1)?;
        Some(self.clock)
    }
    fn get(
        &mut self,
        key: &index::CanonicalKey,
        budget: &mut budget::MaintenanceBudget<'_>,
    ) -> Lookup {
        if !budget.charge(budget::tree_work(self.entries.len(), index::INDEX_BYTES)) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return Lookup::Bypass;
        }
        let Some(age) = self.tick() else {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return Lookup::Bypass;
        };
        let Some(bucket) = self.entries.get(&key.digest) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return Lookup::Miss;
        };
        if !budget.charge(index::equality_work(bucket.len(), key.bytes.len())) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return Lookup::Bypass;
        }
        // The digest only selects a bounded bucket. Full canonical equality is
        // the sole reason a stored answer may replace the original oracle.
        let Some(position) = bucket
            .iter()
            .position(|entry| entry.key.as_ref() == key.bytes.as_slice())
        else {
            if bucket.len() == index::BUCKET_LIMIT {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                return Lookup::Bypass;
            }
            self.stats.misses = self.stats.misses.saturating_add(1);
            return Lookup::Miss;
        };
        let entry = &bucket[position];
        let upkeep = budget::tree_work(self.entries.len(), index::INDEX_BYTES)
            .saturating_add(budget::tree_work(self.ages.len(), 0).saturating_mul(2))
            .saturating_add(index::bucket_work())
            .saturating_add(entry.answer.bytes());
        if !budget.charge(upkeep) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return Lookup::Bypass;
        }
        let old_age = entry.age;
        let answer = entry.answer.clone();
        self.ages.remove(&old_age);
        self.ages.insert(age, key.digest);
        self.entries.get_mut(&key.digest).expect("retained bucket")[position].age = age;
        Lookup::Hit(answer)
    }
    fn insert(
        &mut self,
        key: index::CanonicalKey,
        answer: Answer,
        budget: &mut budget::MaintenanceBudget<'_>,
    ) {
        if crate::current_cancellation().is_cancelled() {
            return;
        }
        let payload = key.bytes.len().saturating_add(answer.bytes());
        let charge = payload.saturating_add(ENTRY_METADATA);
        if self.limits.entries == 0 || charge > self.limits.retained_bytes {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        }
        // Recheck admission before eviction, including full equality. A full
        // collision bucket bypasses rather than evicting unrelated entries to
        // fit an unbounded sequence of keys with the same routing digest.
        if !budget.charge(budget::tree_work(self.entries.len(), index::INDEX_BYTES)) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        }
        if let Some(bucket) = self.entries.get(&key.digest) {
            if !budget.charge(index::equality_work(bucket.len(), key.bytes.len())) {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                return;
            }
            if bucket
                .iter()
                .any(|entry| entry.key.as_ref() == key.bytes.as_slice())
            {
                return;
            }
            if bucket.len() == index::BUCKET_LIMIT {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                return;
            }
        }
        while self.stats.entries >= self.limits.entries
            || self.stats.retained_bytes.saturating_add(charge) > self.limits.retained_bytes
        {
            if !budget.charge(budget::tree_work(self.ages.len(), 0)) {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                return;
            }
            let upkeep = budget::tree_work(self.entries.len(), index::INDEX_BYTES)
                .saturating_mul(2)
                .saturating_add(budget::tree_work(self.ages.len(), 0))
                .saturating_add(index::bucket_work());
            if !budget.charge(upkeep) {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                return;
            }
            let Some((age, digest)) = self.ages.pop_first() else {
                return;
            };
            let bucket = self.entries.get_mut(&digest).expect("LRU bucket");
            let position = bucket
                .iter()
                .position(|entry| entry.age == age)
                .expect("LRU age");
            let entry = bucket.remove(position);
            let empty = bucket.is_empty();
            if empty {
                self.entries.remove(&digest);
            }
            self.stats.entries -= 1;
            self.stats.retained_bytes -= entry.charge;
            self.stats.payload_bytes -= entry.payload;
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        let upkeep = budget::tree_work(self.entries.len(), index::INDEX_BYTES)
            .saturating_add(budget::tree_work(self.ages.len(), 0))
            .saturating_add(index::bucket_work())
            .saturating_add(key.bytes.len());
        if !budget.charge(upkeep) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        }
        let Some(age) = self.tick() else {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        };
        let canonical: Arc<[u8]> = key.bytes.into_boxed_slice().into();
        self.ages.insert(age, key.digest);
        self.entries
            .entry(key.digest)
            .or_insert_with(|| Vec::with_capacity(index::BUCKET_LIMIT))
            .push(Entry {
                key: canonical,
                answer,
                age,
                charge,
                payload,
            });
        self.stats.entries += 1;
        self.stats.payload_bytes += payload;
        self.stats.retained_bytes += charge;
        self.stats.insertions = self.stats.insertions.saturating_add(1);
    }
    fn remember_unique(
        &mut self,
        key: index::CanonicalKey,
        answer: &UniqueOutcome,
        budget: &mut budget::MaintenanceBudget<'_>,
    ) {
        let answer_bytes = match answer {
            UniqueOutcome::Unique(value) => value.len(),
            _ => 0,
        };
        // Admission precedes cloning a possibly very wide native literal.
        if key
            .bytes
            .len()
            .saturating_add(answer_bytes)
            .saturating_add(ENTRY_METADATA)
            > self.limits.retained_bytes
        {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        }
        if !budget.charge(answer_bytes) {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
            return;
        }
        self.insert(key, Answer::Unique(answer.clone()), budget);
    }
    fn key(
        &mut self,
        base: &[Bool],
        model: Option<&Bv>,
        provider: Provider,
        rlimit: u32,
        scope: CacheScope,
        budget: &mut budget::MaintenanceBudget<'_>,
    ) -> Option<index::CanonicalKey> {
        match key::query(base, model, provider, rlimit, scope, self.limits, budget) {
            Ok(bytes) => {
                let Some(digest) = index::digest(&bytes, budget) else {
                    self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                    return None;
                };
                #[cfg(test)]
                let digest = if self.force_collisions {
                    index::collision()
                } else {
                    digest
                };
                Some(index::CanonicalKey { bytes, digest })
            }
            Err(_) => {
                self.stats.bypasses = self.stats.bypasses.saturating_add(1);
                None
            }
        }
    }
}
fn cancellation() -> bool {
    crate::current_cancellation().is_cancelled()
}
/// A mathematical unique/multiple/infeasible answer. Arbitrary model SAT values
/// are never installed; Unique is recorded only after its independent exclusion
/// check proved UNSAT. The caller keeps its original prepaid query charges.
pub fn check_unique_cached(
    base: &[Bool],
    value: &Bv,
    provider: Provider,
    rlimit: u32,
    query: &mut CacheQuery<'_>,
) -> UniqueOutcome {
    if cancellation() || rlimit == 0 {
        return crate::check_unique(base, value, provider, rlimit);
    }
    let cache = &mut query.cache;
    let mut key = cache.key(
        base,
        Some(value),
        provider,
        rlimit,
        query.scope,
        &mut query.budget,
    );
    if cancellation() {
        return UniqueOutcome::Unknown(Unknown::Cancelled);
    }
    if let Some(ref bytes) = key {
        match cache.get(bytes, &mut query.budget) {
            Lookup::Hit(Answer::Unique(answer)) => {
                if cancellation() {
                    return UniqueOutcome::Unknown(Unknown::Cancelled);
                }
                cache.stats.unique_hits = cache.stats.unique_hits.saturating_add(1);
                return answer;
            }
            Lookup::Miss => {}
            _ => key = None,
        }
    }
    let answer = crate::check_unique(base, value, provider, rlimit);
    if cancellation() {
        return UniqueOutcome::Unknown(Unknown::Cancelled);
    }
    match &answer {
        UniqueOutcome::Unique(_) | UniqueOutcome::Multiple | UniqueOutcome::Infeasible => {
            if let Some(key) = key {
                cache.remember_unique(key, &answer, &mut query.budget)
            }
        }
        _ => cache.stats.undecided = cache.stats.undecided.saturating_add(1),
    }
    if cancellation() {
        return UniqueOutcome::Unknown(Unknown::Cancelled);
    }
    answer
}
pub(crate) fn candidate_current(
    base: &[Bool],
    provider: Provider,
    rlimit: u32,
    query: &mut CacheQuery<'_>,
) -> Outcome {
    if cancellation() || rlimit == 0 {
        return crate::query_pair::checked_current(base, None, provider, rlimit);
    }
    let cache = &mut query.cache;
    let mut key = cache.key(base, None, provider, rlimit, query.scope, &mut query.budget);
    if cancellation() {
        return Outcome::Unknown(Unknown::Cancelled);
    }
    if let Some(ref bytes) = key {
        match cache.get(bytes, &mut query.budget) {
            Lookup::Hit(Answer::Candidate(sat)) => {
                if cancellation() {
                    return Outcome::Unknown(Unknown::Cancelled);
                }
                cache.stats.candidate_hits = cache.stats.candidate_hits.saturating_add(1);
                return if sat {
                    Outcome::Sat(None)
                } else {
                    Outcome::Unsat
                };
            }
            Lookup::Miss => {}
            _ => key = None,
        }
    }
    let answer = crate::query_pair::checked_current(base, None, provider, rlimit);
    if cancellation() {
        return Outcome::Unknown(Unknown::Cancelled);
    }
    match answer {
        Outcome::Sat(_) | Outcome::Unsat => {
            if let Some(key) = key {
                cache.insert(
                    key,
                    Answer::Candidate(matches!(answer, Outcome::Sat(_))),
                    &mut query.budget,
                )
            }
        }
        _ => cache.stats.undecided = cache.stats.undecided.saturating_add(1),
    }
    if cancellation() {
        return Outcome::Unknown(Unknown::Cancelled);
    }
    answer
}
