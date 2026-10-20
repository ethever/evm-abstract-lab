//! Complete word covers used by executable target and extent dispatch.
//!
//! A domain can know one exact word without retaining finite membership.
//! Preserve that knowledge without allocating a temporary candidate set.

use crate::{
    U256,
    domain::{EvmValue, WordElement},
};
use std::collections::BTreeSet;

pub(super) struct Candidates<'a> {
    finite: Option<&'a BTreeSet<U256>>,
    singleton: Option<U256>,
}

impl Candidates<'_> {
    pub(super) fn iter(&self) -> impl Iterator<Item = &U256> {
        self.finite
            .into_iter()
            .flatten()
            .chain(self.singleton.iter())
    }

    pub(super) fn len(&self) -> usize {
        self.finite.map_or(1, BTreeSet::len)
    }

    pub(super) fn contains(&self, word: &U256) -> bool {
        self.finite.map_or_else(
            || self.singleton.as_ref() == Some(word),
            |set| set.contains(word),
        )
    }
}

pub(super) fn complete<N: WordElement>(value: &EvmValue<N>) -> Option<Candidates<'_>> {
    if let Some(finite) = value.constants() {
        return Some(Candidates {
            finite: Some(finite),
            singleton: None,
        });
    }
    value.singleton().map(|word| Candidates {
        finite: None,
        singleton: Some(word),
    })
}
