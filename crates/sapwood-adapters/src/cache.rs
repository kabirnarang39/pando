//! Tiering building blocks.
//!
//! * [`BoundedCache`]: memory-bounded object cache (quick_cache: sharded, CLOCK-Pro style
//!   eviction that resists scans, weighted by payload bytes).
//! * [`Tiered`]: read-through/write-through composition of a local store with an
//!   upstream store. Server, regional cache and node daemon are all just different
//!   `(local, upstream)` pairs behind the same `Objects` RPC.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use quick_cache::Weighter;
use quick_cache::sync::Cache;
use sapwood_core::ports::ObjectStore;
use sapwood_core::{Id, Result};

#[derive(Clone)]
struct PayloadWeight;
impl Weighter<Id, Bytes> for PayloadWeight {
    fn weight(&self, _: &Id, v: &Bytes) -> u64 {
        // +64 approximates per-entry overhead so tiny objects still count.
        v.len() as u64 + 64
    }
}

/// Memory-only object store with a byte budget. Evicted objects are simply gone, so
/// use it as the `local` half of a [`Tiered`] store.
pub struct BoundedCache {
    cache: Cache<Id, Bytes, PayloadWeight>,
}

impl BoundedCache {
    pub fn new(capacity_bytes: u64) -> Self {
        // Estimate ~8 KiB per object for initial table sizing.
        let est_items = (capacity_bytes / 8192).max(1024) as usize;
        Self { cache: Cache::with_weighter(est_items, capacity_bytes, PayloadWeight) }
    }
    pub fn weight(&self) -> u64 {
        self.cache.weight()
    }
}

#[async_trait]
impl ObjectStore for BoundedCache {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        Ok(self.cache.get(id))
    }
    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        self.cache.insert(id, payload);
        Ok(())
    }
    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.cache.contains_key(id))
    }
    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        Ok(ids.iter().map(|id| self.cache.get(id)).collect())
    }
}

/// Hit/miss counters for a tier.
#[derive(Default, Debug)]
pub struct TierStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl TierStats {
    pub fn hit_rate(&self) -> f64 {
        let (h, m) = (self.hits.load(Ordering::Relaxed), self.misses.load(Ordering::Relaxed));
        if h + m == 0 { 0.0 } else { h as f64 / (h + m) as f64 }
    }
}

/// Read-through, write-through tier. Objects are immutable, so there is no
/// invalidation: a cached object is correct forever.
pub struct Tiered {
    pub local: Arc<dyn ObjectStore>,
    pub upstream: Arc<dyn ObjectStore>,
    pub stats: Arc<TierStats>,
}

impl Tiered {
    pub fn new(local: Arc<dyn ObjectStore>, upstream: Arc<dyn ObjectStore>) -> Self {
        Self { local, upstream, stats: Arc::default() }
    }
}

#[async_trait]
impl ObjectStore for Tiered {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        Ok(self.get_many(std::slice::from_ref(id)).await?.pop().flatten())
    }

    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        // Upstream first: an acknowledged write must be durable at the authority.
        self.upstream.put(id, payload.clone()).await?;
        self.local.put(id, payload).await
    }

    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.local.has(id).await? || self.upstream.has(id).await?)
    }

    /// One local batch read, then one upstream batch for all misses.
    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        let mut out = self.local.get_many(ids).await?;
        let miss_idx: Vec<usize> = (0..ids.len()).filter(|&i| out[i].is_none()).collect();
        self.stats.hits.fetch_add((ids.len() - miss_idx.len()) as u64, Ordering::Relaxed);
        self.stats.misses.fetch_add(miss_idx.len() as u64, Ordering::Relaxed);
        if miss_idx.is_empty() {
            return Ok(out);
        }
        let miss_ids: Vec<Id> = miss_idx.iter().map(|&i| ids[i]).collect();
        let fetched = self.upstream.get_many(&miss_ids).await?;
        for (i, payload) in miss_idx.into_iter().zip(fetched) {
            if let Some(p) = &payload {
                self.local.put(ids[i], p.clone()).await?;
            }
            out[i] = payload;
        }
        Ok(out)
    }

    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let local_missing = self.local.missing(ids).await?;
        if local_missing.is_empty() {
            return Ok(local_missing);
        }
        self.upstream.missing(&local_missing).await
    }
}
