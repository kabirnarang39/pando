//! Infrastructure ports. Every storage or network detail is reached only through these
//! traits, so adapters (memory, SQLite, Postgres, S3/GCS/Azure, an upstream tier...) are
//! interchangeable without touching domain code.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt, stream};

use crate::{Id, Result};

/// Concurrency used by default batch implementations.
pub const BATCH_CONCURRENCY: usize = 64;

/// Content-addressed object storage. Payloads are opaque `kind || zstd(content)` bytes.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>>;
    async fn put(&self, id: Id, payload: Bytes) -> Result<()>;
    async fn has(&self, id: &Id) -> Result<bool>;

    /// Batch fetch, results aligned with `ids`. Override when the backend can do better
    /// than bounded concurrent point reads.
    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        stream::iter(ids.iter().copied())
            .map(|id| async move { self.get(&id).await })
            .buffered(BATCH_CONCURRENCY)
            .try_collect()
            .await
    }

    /// Batch write. Override to use one transaction / multi-row insert.
    async fn put_many(&self, items: Vec<(Id, Bytes)>) -> Result<()> {
        stream::iter(items).map(|(id, p)| self.put(id, p)).buffer_unordered(BATCH_CONCURRENCY).try_collect::<()>().await
    }

    /// Ids from `ids` this store cannot serve.
    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let present: Vec<bool> = stream::iter(ids.iter().copied())
            .map(|id| async move { self.has(&id).await })
            .buffered(BATCH_CONCURRENCY)
            .try_collect()
            .await?;
        Ok(ids.iter().zip(present).filter(|(_, p)| !p).map(|(id, _)| *id).collect())
    }
}

/// Append-only write-ahead log of changesets, keyed by changeset id.
#[async_trait]
pub trait ChangesetLog: Send + Sync {
    /// Idempotent: appending an existing id is a no-op.
    async fn append(&self, id: Id, encoded: Bytes) -> Result<()>;
    async fn get(&self, id: &Id) -> Result<Option<Bytes>>;
}

/// Named pointers. The only mutable state in the system; every move is a
/// compare-and-swap on one key, so there is no global lock.
#[async_trait]
pub trait RefStore: Send + Sync {
    async fn get(&self, name: &str) -> Result<Option<Id>>;
    /// Set `name` to `new` iff it currently equals `expected` (`None` = absent).
    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool>;
    async fn list(&self) -> Result<Vec<(String, Id)>>;
}

/// Storage for derived data, keyed by (derivation kind, changeset id).
#[async_trait]
pub trait DerivedStore: Send + Sync {
    async fn get(&self, kind: &str, id: &Id) -> Result<Option<Bytes>>;
    async fn put(&self, kind: &str, id: Id, value: Bytes) -> Result<()>;
}
