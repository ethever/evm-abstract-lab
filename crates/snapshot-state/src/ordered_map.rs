use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use std::ops::{Bound, Index, RangeBounds};

#[cfg(not(feature = "imbl"))]
type Map<K, V> = std::collections::BTreeMap<K, V>;
#[cfg(feature = "imbl")]
type Map<K, V> = imbl::OrdMap<K, V>;

/// An ordered map whose cloned entry versions remain independent after mutation.
///
/// Keys iterate in ascending order and serialize in that same order for both
/// backends. Only this wrapper's operations are exposed, so callers do not rely
/// on a backend's entry or iterator types. Keys and values must be cloneable to
/// support persistent copy-on-write updates. Cloning a std map copies its entries;
/// cloning an imbl map shares its tree until a version is modified.
/// `Clone` must isolate writable key and value contents: shared mutable handles
/// retain shared pointees and therefore do not provide rollback isolation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedMap<K: Ord + Clone, V: Clone> {
    inner: Map<K, V>,
}

impl<K: Ord + Clone, V: Clone> OrderedMap<K, V> {
    /// Construct an empty map.
    pub fn new() -> Self {
        Self { inner: Map::new() }
    }

    /// Number of explicit entries in this version.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether this version has no explicit entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Read an explicit value without modifying this version.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    /// Mutably access an explicit value, isolating any shared version first.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    /// Replace a value, returning its previous value if the key was present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Remove a key, returning its previous value if present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    /// Remove all explicit entries from this version.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Iterate over explicit entries in ascending key order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            inner: self.inner.iter(),
        }
    }

    /// Iterate over explicit keys in ascending order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }

    /// Iterate over explicit values in ascending key order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }

    /// Read entries within a key range, in ascending order.
    ///
    /// # Panics
    ///
    /// Panics if the start is greater than the end, or both bounds exclude the
    /// same key. Bounds are checked even when the map is empty.
    pub fn range<R: RangeBounds<K>>(&self, range: R) -> impl Iterator<Item = (&K, &V)> {
        validate_range(&range);
        self.inner.range(range)
    }

    /// Update values within a key range, visiting them in ascending order.
    ///
    /// Shared versions remain unchanged. The std backend uses a mutable range;
    /// imbl collects only the selected keys before copy-on-write mutation.
    ///
    /// # Panics
    ///
    /// Invalid bounds panic under the same rules as [`Self::range`].
    pub fn update_range<R, F>(&mut self, range: R, mut update: F)
    where
        R: RangeBounds<K>,
        F: FnMut(&K, &mut V),
    {
        validate_range(&range);
        #[cfg(not(feature = "imbl"))]
        for (key, value) in self.inner.range_mut(range) {
            update(key, value);
        }
        #[cfg(feature = "imbl")]
        {
            let keys: Vec<_> = self
                .inner
                .range(range)
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys {
                update(
                    &key,
                    self.inner
                        .get_mut(&key)
                        .expect("collected key remains present"),
                );
            }
        }
    }

    /// Update every explicit value in ascending key order.
    pub fn update_values<F: FnMut(&mut V)>(&mut self, mut update: F) {
        #[cfg(not(feature = "imbl"))]
        for value in self.inner.values_mut() {
            update(value);
        }
        #[cfg(feature = "imbl")]
        {
            let keys: Vec<_> = self.inner.keys().cloned().collect();
            for key in keys {
                update(
                    self.inner
                        .get_mut(&key)
                        .expect("collected key remains present"),
                );
            }
        }
    }

    /// Visit entries in ascending order, keeping only those accepted by a predicate.
    ///
    /// The predicate can update a retained value; every mutation stays local to
    /// this map version even when its backing nodes are shared with a checkpoint.
    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, predicate: F) {
        #[cfg(not(feature = "imbl"))]
        self.inner.retain(predicate);
        #[cfg(feature = "imbl")]
        {
            let mut predicate = predicate;
            let keys: Vec<_> = self.inner.keys().cloned().collect();
            for key in keys {
                if !predicate(
                    &key,
                    self.inner
                        .get_mut(&key)
                        .expect("collected key remains present"),
                ) {
                    self.inner.remove(&key);
                }
            }
        }
    }
}

fn validate_range<K: Ord, R: RangeBounds<K>>(range: &R) {
    match (range.start_bound(), range.end_bound()) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => {}
        (
            Bound::Included(start) | Bound::Excluded(start),
            Bound::Included(end) | Bound::Excluded(end),
        ) => {
            assert!(start <= end, "range start is greater than range end");
            assert!(
                start != end
                    || !matches!(
                        (range.start_bound(), range.end_bound()),
                        (Bound::Excluded(_), Bound::Excluded(_))
                    ),
                "range start and end are equal and excluded"
            );
        }
    }
}

impl<K: Ord + Clone, V: Clone> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Clone + Serialize, V: Clone + Serialize> Serialize for OrderedMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (key, value) in self {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        map.extend(iter);
        map
    }
}

impl<K: Ord + Clone, V: Clone> Extend<(K, V)> for OrderedMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl<K: Ord + Clone, V: Clone> Index<&K> for OrderedMap<K, V> {
    type Output = V;

    fn index(&self, key: &K) -> &Self::Output {
        self.get(key).expect("no entry found for key")
    }
}

/// A borrowed map iterator, independent of the selected backend's public types.
pub struct Iter<'a, K: Ord + Clone, V: Clone> {
    inner: <&'a Map<K, V> as IntoIterator>::IntoIter,
}

impl<'a, K: Ord + Clone, V: Clone> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K: Ord + Clone, V: Clone> IntoIterator for &'a OrderedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An owned map iterator, independent of the selected backend's public types.
pub struct IntoIter<K: Ord + Clone, V: Clone> {
    inner: <Map<K, V> as IntoIterator>::IntoIter,
}

impl<K: Ord + Clone, V: Clone> Iterator for IntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K: Ord + Clone, V: Clone> IntoIterator for OrderedMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            inner: self.inner.into_iter(),
        }
    }
}
