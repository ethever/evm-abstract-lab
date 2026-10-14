//! A typed lease of the caller's existing ledger, with no refund or global state.
use crate::{CacheQuery, CacheScope, Cancellation, DecidedCache};

/// Borrow the same accounting fields used by the caller's ordinary charge method.
/// One permit may be reserved before query encoding; consuming it does not charge twice.
#[derive(Debug)]
pub struct QueryBudget<'a> {
    maximum: usize,
    consumed: &'a mut usize,
    exhausted: &'a mut bool,
    cancellation: Cancellation,
    cost: usize,
    reserved: bool,
    cache: Option<(&'a mut DecidedCache, CacheScope)>,
}
impl<'a> QueryBudget<'a> {
    /// The caller supplies its original per-query cost and cancellation identity.
    pub fn new(
        maximum: usize,
        consumed: &'a mut usize,
        exhausted: &'a mut bool,
        cancellation: Cancellation,
        cost: usize,
    ) -> Self {
        Self {
            maximum,
            consumed,
            exhausted,
            cancellation,
            cost,
            reserved: false,
            cache: None,
        }
    }
    /// Attach the analysis owner's cache without copying/refunding its ledger.
    pub fn with_cache(mut self, cache: &'a mut DecidedCache, scope: CacheScope) -> Self {
        self.cache = Some((cache, scope));
        self
    }
    pub(crate) fn cache(&mut self) -> Option<CacheQuery<'_>> {
        self.cache.as_mut().map(|(cache, scope)| {
            CacheQuery::new(
                self.maximum,
                self.consumed,
                self.cancellation.clone(),
                cache,
                *scope,
            )
            .with_allowance(self.cost / 4)
        })
    }
    /// Reserve the next query before encoding or native construction.
    /// A reservation already held for this query is reused. There is no refund.
    pub fn reserve(&mut self) -> bool {
        if self.reserved {
            return true;
        }
        if self.cancellation.is_cancelled() {
            *self.exhausted = true;
            return false;
        }
        if self.cost > self.maximum.saturating_sub(*self.consumed) {
            *self.exhausted = true;
            return false;
        }
        *self.consumed += self.cost;
        self.reserved = true;
        true
    }
    /// Consume the one reserved operation or reserve its original cost now.
    /// Encoding failure or an immediate logical result consumes the same permit
    /// without inventing a native query or refunding previously reserved work.
    pub fn consume(&mut self) -> bool {
        if !self.reserve() {
            return false;
        }
        self.reserved = false;
        true
    }
}
