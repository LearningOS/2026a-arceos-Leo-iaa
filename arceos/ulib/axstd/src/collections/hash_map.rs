//! A simple hash map built on top of `alloc`.
//!
//! The table is an array of buckets (separate chaining): a key is hashed,
//! the hash picks a bucket, and the bucket stores all `(key, value)` pairs
//! that landed there. When the average bucket length exceeds a threshold the
//! table doubles and every entry is redistributed, so lookups stay O(1) on
//! average.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt;
use core::hash::{BuildHasher, Hash, Hasher};
use core::sync::atomic::{AtomicU64, Ordering};

/// Initial number of buckets (must be a power of two).
const INITIAL_BUCKETS: usize = 16;
/// Grow when `len > buckets * MAX_LOAD_FACTOR`.
const MAX_LOAD_FACTOR: usize = 1;

/// The default hasher: 64-bit FNV-1a with a per-map seed and a final mixing
/// step, so that the low bits used for bucket selection are well spread.
#[derive(Clone, Debug)]
pub struct DefaultHasher {
    state: u64,
}

impl DefaultHasher {
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

    fn with_seed(seed: u64) -> Self {
        Self {
            state: Self::FNV_OFFSET ^ seed,
        }
    }
}

impl Default for DefaultHasher {
    fn default() -> Self {
        Self::with_seed(0)
    }
}

impl Hasher for DefaultHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(Self::FNV_PRIME);
        }
    }

    fn finish(&self) -> u64 {
        // splitmix64 finalizer: avalanches all input bits into the output.
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Creates [`DefaultHasher`]s; each `RandomState` gets a different seed.
#[derive(Clone, Debug)]
pub struct RandomState {
    seed: u64,
}

impl RandomState {
    /// Creates a new `RandomState` seeded from the clock and a counter.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let now = arceos_api::time::ax_wall_time().as_nanos() as u64;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            seed: now ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        }
    }
}

impl Default for RandomState {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = DefaultHasher;

    fn build_hasher(&self) -> DefaultHasher {
        DefaultHasher::with_seed(self.seed)
    }
}

/// A hash map implemented with separate chaining.
pub struct HashMap<K, V, S = RandomState> {
    buckets: Vec<Vec<(K, V)>>,
    len: usize,
    hash_builder: S,
}

impl<K, V> HashMap<K, V, RandomState> {
    /// Creates an empty `HashMap`.
    pub fn new() -> Self {
        Self::with_hasher(RandomState::new())
    }

    /// Creates an empty `HashMap` with room for at least `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_hasher(capacity, RandomState::new())
    }
}

impl<K, V, S> HashMap<K, V, S> {
    /// Creates an empty `HashMap` which will use `hash_builder` to hash keys.
    pub fn with_hasher(hash_builder: S) -> Self {
        Self {
            buckets: Vec::new(),
            len: 0,
            hash_builder,
        }
    }

    /// Creates an empty `HashMap` with the given capacity and hasher.
    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        let mut map = Self::with_hasher(hash_builder);
        if capacity > 0 {
            let n = (capacity / MAX_LOAD_FACTOR).max(INITIAL_BUCKETS).next_power_of_two();
            map.buckets = new_buckets(n);
        }
        map
    }

    /// Returns the number of entries in the map.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the map contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes all entries, keeping the allocated buckets.
    pub fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.len = 0;
    }

    /// Returns a reference to the map's hasher builder.
    pub fn hasher(&self) -> &S {
        &self.hash_builder
    }

    /// An iterator over all `(&K, &V)` pairs, in arbitrary order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            buckets: self.buckets.iter(),
            current: [].iter(),
            remaining: self.len,
        }
    }

    /// An iterator over all `(&K, &mut V)` pairs, in arbitrary order.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut {
            remaining: self.len,
            buckets: self.buckets.iter_mut(),
            current: [].iter_mut(),
        }
    }

    /// An iterator over all keys.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    /// An iterator over all values.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    /// An iterator over mutable references to all values.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.iter_mut().map(|(_, v)| v)
    }
}

impl<K: Hash + Eq, V, S: BuildHasher> HashMap<K, V, S> {
    /// Maps a key to a bucket index. `buckets.len()` must be a non-zero
    /// power of two, so masking with `len - 1` equals `hash % len`.
    fn bucket_index<Q: Hash + ?Sized>(&self, key: &Q) -> usize {
        let hash = self.hash_builder.hash_one(key);
        (hash as usize) & (self.buckets.len() - 1)
    }

    /// Doubles the bucket array and re-inserts every entry.
    fn grow(&mut self) {
        let new_len = (self.buckets.len() * 2).max(INITIAL_BUCKETS);
        let old = core::mem::replace(&mut self.buckets, new_buckets(new_len));
        for (k, v) in old.into_iter().flatten() {
            let idx = self.bucket_index(&k);
            self.buckets[idx].push((k, v));
        }
    }

    /// Inserts a key-value pair. Returns the old value if the key was present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.buckets.is_empty() {
            self.grow();
        }
        let idx = self.bucket_index(&key);
        if let Some((_, v)) = self.buckets[idx].iter_mut().find(|(k, _)| *k == key) {
            return Some(core::mem::replace(v, value));
        }
        self.buckets[idx].push((key, value));
        self.len += 1;
        if self.len > self.buckets.len() * MAX_LOAD_FACTOR {
            self.grow();
        }
        None
    }

    /// Returns a reference to the value of `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get_key_value(key).map(|(_, v)| v)
    }

    /// Returns the stored key and a reference to its value.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let idx = self.bucket_index(key);
        self.buckets[idx]
            .iter()
            .find(|(k, _)| k.borrow() == key)
            .map(|(k, v)| (k, v))
    }

    /// Returns a mutable reference to the value of `key`.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let idx = self.bucket_index(key);
        self.buckets[idx]
            .iter_mut()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    /// Returns `true` if the map contains `key`.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Removes `key` from the map, returning its value if it was present.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let idx = self.bucket_index(key);
        let bucket = &mut self.buckets[idx];
        let pos = bucket.iter().position(|(k, _)| k.borrow() == key)?;
        self.len -= 1;
        // Order inside a bucket does not matter, so O(1) swap_remove is fine.
        Some(bucket.swap_remove(pos).1)
    }
}

fn new_buckets<K, V>(n: usize) -> Vec<Vec<(K, V)>> {
    let mut buckets = Vec::with_capacity(n);
    buckets.resize_with(n, Vec::new);
    buckets
}

impl<K, V, S: Default> Default for HashMap<K, V, S> {
    fn default() -> Self {
        Self::with_hasher(S::default())
    }
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for HashMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Hash + Eq, V, S: BuildHasher> Extend<(K, V)> for HashMap<K, V, S> {
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<K: Hash + Eq, V> FromIterator<(K, V)> for HashMap<K, V, RandomState> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = HashMap::new();
        map.extend(iter);
        map
    }
}

impl<'a, K, V, S> IntoIterator for &'a HashMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut HashMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Iterator over `(&K, &V)`, returned by [`HashMap::iter`].
pub struct Iter<'a, K, V> {
    buckets: core::slice::Iter<'a, Vec<(K, V)>>,
    current: core::slice::Iter<'a, (K, V)>,
    remaining: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((k, v)) = self.current.next() {
                self.remaining -= 1;
                return Some((k, v));
            }
            // Current bucket exhausted: move on to the next one.
            self.current = self.buckets.next()?.iter();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

/// Iterator over `(&K, &mut V)`, returned by [`HashMap::iter_mut`].
pub struct IterMut<'a, K, V> {
    buckets: core::slice::IterMut<'a, Vec<(K, V)>>,
    current: core::slice::IterMut<'a, (K, V)>,
    remaining: usize,
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((k, v)) = self.current.next() {
                self.remaining -= 1;
                return Some((&*k, v));
            }
            self.current = self.buckets.next()?.iter_mut();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
