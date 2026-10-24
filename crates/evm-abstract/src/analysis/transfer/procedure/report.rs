//! Move already paid root materializations into their own read-only reports.
//!
//! This cache lives for one execute_with call. Producer revisions identify the
//! exact immutable output that supplied each array, independently of numeric
//! equality. No report value is reused for executable instantiation.

use super::{EvmByteArray, EvmByteArraySummary, OutcomeKind, WordElement, WorkBudget};
use crate::resource::{
    CloneError,
    clone_payment::{checkpoint, pay},
};
use std::{iter::Peekable, vec::IntoIter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Source {
    pub namespace: usize,
    pub procedure: usize,
    pub entry_revision: usize,
    pub kind: OutcomeKind,
    pub generation: usize,
}

const SOURCE_WORK: usize = std::mem::size_of::<Source>().div_ceil(std::mem::size_of::<usize>());
const RETAIN_WORK: usize = SOURCE_WORK * 2;
const TAKE_WORK: usize = SOURCE_WORK * 2 + 4;

pub(super) struct Data<N: WordElement> {
    source: Source,
    pub bound: EvmByteArraySummary<N>,
    pub data: EvmByteArray<N>,
}

impl<N: WordElement> Data<N> {
    pub(super) fn new(
        source: Source,
        bound: EvmByteArraySummary<N>,
        data: EvmByteArray<N>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        // Existing summary/array Clone invoices own their complete retention
        // and release. Only the producer stamp is additional retained data.
        pay(budget, RETAIN_WORK)?;
        Ok(Self {
            source,
            bound,
            data,
        })
    }
}

pub(super) fn take_data<N: WordElement>(
    sources: &mut Peekable<IntoIter<Data<N>>>,
    source: Source,
    budget: &mut WorkBudget,
) -> Result<Option<EvmByteArray<N>>, CloneError> {
    // One fixed stamp comparison and iterator move, with no allocation,
    // native Eq/Clone/query, byte replay or new symbolic source construction.
    pay(budget, TAKE_WORK)?;
    let data = if sources.peek().is_some_and(|entry| entry.source == source) {
        Some(sources.next().expect("peeked report source").data)
    } else {
        None
    };
    checkpoint(budget)?;
    Ok(data)
}

#[cfg(test)]
mod tests;
