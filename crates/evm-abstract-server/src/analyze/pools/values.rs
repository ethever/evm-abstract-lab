//! Full scalar interning keeps byte snapshots compact without stripping facts.
use evm_abstract_protocol::ValueInfo;
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

#[derive(Default)]
pub(super) struct Dictionary {
    values: Vec<ValueInfo>,
    buckets: HashMap<u64, Vec<usize>>,
}
impl Dictionary {
    pub(super) fn intern(&mut self, value: ValueInfo) -> usize {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        self.insert(hasher.finish(), value)
    }
    fn insert(&mut self, hash: u64, value: ValueInfo) -> usize {
        let bucket = self.buckets.entry(hash).or_default();
        // The digest only selects candidates. Every retained component must
        // compare equal before sharing a value, including provenance and IDs.
        if let Some(index) = bucket
            .iter()
            .copied()
            .find(|index| self.values[*index] == value)
        {
            return index;
        }
        let index = self.values.len();
        self.values.push(value);
        bucket.push(index);
        index
    }
    pub(super) fn into_values(self) -> Vec<ValueInfo> {
        self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_collision_never_merges_distinct_scalar_facts() {
        let concrete = crate::analyze::value::info(&evm_abstract::domain::AbstractValue::constant(
            evm_abstract::U256::from(0xab),
        ));
        let mut memory = concrete.clone();
        memory.origins = Some(vec![evm_abstract_protocol::ValueOrigin::Memory]);
        let mut dictionary = Dictionary::default();
        let first = dictionary.insert(42, concrete.clone());
        let second = dictionary.insert(42, memory.clone());
        assert_ne!(first, second);
        assert_eq!(dictionary.insert(42, concrete), first);
        assert_eq!(dictionary.insert(42, memory), second);
        assert_eq!(dictionary.into_values().len(), 2);
    }
}
