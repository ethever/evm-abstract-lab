//! Borrowed canonical facts for paid application projection, without replay.

use super::{EvmStoreSummary, EvmValue, OrderedMap, SummaryPlane, WordElement};
use crate::world::{Code, EvmByteArraySummary, Existence, LogKey};
use alloy_primitives::{Address, U256};

/// Immutable plane vocabulary. Missing cells use owner then global defaults.
pub struct EvmPlaneSummaryView<'a, N: WordElement> {
    /// Explicit physical owner/slot facts.
    pub slots: &'a OrderedMap<(Address, U256), EvmValue<N>>,
    /// Explicit physical owner defaults.
    pub defaults: &'a OrderedMap<Address, EvmValue<N>>,
    /// Default at every other coordinate.
    pub global_default: &'a EvmValue<N>,
    /// Independence from initial facts at explicit coordinates.
    pub independent_slots: &'a OrderedMap<(Address, U256), bool>,
    /// Independence for otherwise unlisted slots at these owners.
    pub independent_owners: &'a OrderedMap<Address, bool>,
    /// Independence at all other coordinates.
    pub independent_default: bool,
}

impl<N: WordElement> SummaryPlane<N> {
    fn view(&self) -> EvmPlaneSummaryView<'_, N> {
        EvmPlaneSummaryView {
            slots: &self.slots,
            defaults: &self.defaults,
            global_default: &self.global_default,
            independent_slots: &self.initial_independence.slots,
            independent_owners: &self.initial_independence.defaults,
            independent_default: self.initial_independence.global_default,
        }
    }
}

/// Canonical physical public effect bounds. No private roots are instantiated.
pub struct EvmStoreSummaryView<'a, N: WordElement> {
    /// Persistent storage.
    pub persistent: EvmPlaneSummaryView<'a, N>,
    /// Transient storage.
    pub transient: EvmPlaneSummaryView<'a, N>,
    /// Explicit balances.
    pub balances: &'a OrderedMap<Address, EvmValue<N>>,
    /// Balance at every other owner.
    pub balance_default: &'a EvmValue<N>,
    /// Explicit codes; absence means Unknown.
    pub codes: &'a OrderedMap<Address, Code>,
    /// Explicit nonces; absence means Top.
    pub nonces: &'a OrderedMap<Address, EvmValue<N>>,
    /// Explicit presence facts; absence means Unknown.
    pub existence: &'a OrderedMap<Address, Existence>,
    /// Explicit transaction creation facts.
    pub created: &'a OrderedMap<Address, Option<bool>>,
    /// Creation bound at other owners.
    pub created_default: Option<bool>,
    /// Explicit deferred deletion facts.
    pub pending_destruction: &'a OrderedMap<Address, Option<bool>>,
    /// Deferred deletion bound at other owners.
    pub pending_destruction_default: Option<bool>,
    /// Explicit transaction touch facts.
    pub touched: &'a OrderedMap<Address, Option<bool>>,
    /// Touch bound at other owners.
    pub touched_default: Option<bool>,
    /// Additional log sources may occur.
    pub logs_unknown: bool,
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Constant-work borrowed view. The consumer must fund its own enumeration,
    /// scalar observations and output copies before accessing those values.
    pub fn view(&self) -> EvmStoreSummaryView<'_, N> {
        EvmStoreSummaryView {
            persistent: self.persistent.view(),
            transient: self.transient.view(),
            balances: &self.balances,
            balance_default: &self.balance_default,
            codes: &self.codes,
            nonces: &self.nonces,
            existence: &self.existence,
            created: &self.created,
            created_default: self.created_default,
            pending_destruction: &self.pending_destruction,
            pending_destruction_default: self.pending_destruction_default,
            touched: &self.touched,
            touched_default: self.touched_default,
            logs_unknown: self.logs_unknown,
        }
    }

    /// Borrow source-qualified canonical logs; presence is a may-event bound,
    /// with neither sequence nor multiplicity assertions. Traversal is unpaid.
    pub fn logs(&self) -> impl Iterator<Item = (&LogKey, &[EvmValue<N>], &EvmByteArraySummary<N>)> {
        self.possible_logs
            .iter()
            .map(|(source, log)| (source, log.topics.as_slice(), &log.data))
    }

    /// Cached count for funding log iteration before traversal.
    pub fn logs_len(&self) -> usize {
        self.possible_logs.len()
    }
}
