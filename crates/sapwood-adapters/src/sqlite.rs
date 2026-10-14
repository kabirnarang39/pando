//! SQLite adapter for single-node deployments and local development.
//!
//! WAL mode, one writer connection plus a pool of reader connections so reads never
//! block on writes. All SQLite calls run on tokio's blocking pool.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sapwood_core::ports::{ChangesetLog, DerivedStore, ObjectStore, RefStore};
use sapwood_core::{Id, Result};

use crate::storage_err;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS objects    (id BLOB PRIMARY KEY, payload BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS changesets (id BLOB PRIMARY KEY, data BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS refs       (name TEXT PRIMARY KEY, id BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS derived    (kind TEXT NOT NULL, id BLOB NOT NULL, value BLOB NOT NULL,
                                       PRIMARY KEY (kind, id)) WITHOUT ROWID;
";

pub struct SqliteStore {
    writer: Arc<Mutex<Connection>>,
    readers: Arc<Vec<Mutex<Connection>>>,
    next: AtomicUsize,
}

fn tune(c: &Connection) -> rusqlite::Result<()> {
    // NORMAL is durable across application crashes in WAL mode; only an OS crash or
    // power loss can drop the last transactions, never corrupt the database.
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;
         PRAGMA cache_size=-16384; PRAGMA mmap_size=268435456; PRAGMA temp_store=MEMORY;",
    )
}

fn id_of(b: Vec<u8>) -> rusqlite::Result<Id> {
    Id::from_slice(&b).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
}

impl SqliteStore {
    /// Return free pages to the filesystem (after compaction shrank stored objects).
    pub async fn vacuum(&self) -> Result<()> {
        self.write(|c| c.execute_batch("VACUUM")).await
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let writer = Connection::open(path).map_err(storage_err)?;
        tune(&writer).map_err(storage_err)?;
        writer.execute_batch(SCHEMA).map_err(storage_err)?;
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 16);
        let readers = (0..n)
            .map(|_| {
                let c = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                tune(&c)?;
                Ok(Mutex::new(c))
            })
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_err)?;
        Ok(Self { writer: Arc::new(Mutex::new(writer)), readers: Arc::new(readers), next: AtomicUsize::new(0) })
    }

    async fn read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let readers = self.readers.clone();
        let i = self.next.fetch_add(1, Ordering::Relaxed) % readers.len();
        tokio::task::spawn_blocking(move || f(&readers[i].lock().expect("sqlite reader poisoned")))
            .await
            .map_err(storage_err)?
            .map_err(storage_err)
    }

    async fn write<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let writer = self.writer.clone();
        tokio::task::spawn_blocking(move || f(&mut writer.lock().expect("sqlite writer poisoned")))
            .await
            .map_err(storage_err)?
            .map_err(storage_err)
    }
}

#[async_trait]
impl ObjectStore for SqliteStore {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        let id = *id;
        self.read(move |c| {
            c.prepare_cached("SELECT payload FROM objects WHERE id = ?1")?
                .query_row([&id.0[..]], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map(|o| o.map(Bytes::from))
        })
        .await
    }

    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        self.put_many(vec![(id, payload)]).await
    }

    async fn put_many(&self, items: Vec<(Id, Bytes)>) -> Result<()> {
        self.write(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached("INSERT OR IGNORE INTO objects (id, payload) VALUES (?1, ?2)")?;
                for (id, p) in &items {
                    stmt.execute(params![&id.0[..], &p[..]])?;
                }
            }
            tx.commit()
        })
        .await
    }

    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.missing(std::slice::from_ref(id)).await?.is_empty())
    }

    async fn replace(&self, id: Id, payload: Bytes) -> Result<bool> {
        self.write(move |c| {
            c.prepare_cached("UPDATE objects SET payload = ?1 WHERE id = ?2")?
                .execute(params![&payload[..], &id.0[..]])
                .map(|n| n == 1)
        })
        .await
    }

    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        let ids = ids.to_vec();
        self.read(move |c| {
            let mut stmt = c.prepare_cached("SELECT payload FROM objects WHERE id = ?1")?;
            ids.iter()
                .map(|id| {
                    stmt.query_row([&id.0[..]], |r| r.get::<_, Vec<u8>>(0)).optional().map(|o| o.map(Bytes::from))
                })
                .collect()
        })
        .await
    }

    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let ids = ids.to_vec();
        self.read(move |c| {
            let mut stmt = c.prepare_cached("SELECT 1 FROM objects WHERE id = ?1")?;
            let mut out = Vec::new();
            for id in ids {
                if !stmt.exists([&id.0[..]])? {
                    out.push(id);
                }
            }
            Ok(out)
        })
        .await
    }
}

#[async_trait]
impl ChangesetLog for SqliteStore {
    async fn append(&self, id: Id, encoded: Bytes) -> Result<()> {
        self.write(move |c| {
            c.prepare_cached("INSERT OR IGNORE INTO changesets (id, data) VALUES (?1, ?2)")?
                .execute(params![&id.0[..], &encoded[..]])
                .map(drop)
        })
        .await
    }

    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        let id = *id;
        self.read(move |c| {
            c.prepare_cached("SELECT data FROM changesets WHERE id = ?1")?
                .query_row([&id.0[..]], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map(|o| o.map(Bytes::from))
        })
        .await
    }
}

#[async_trait]
impl RefStore for SqliteStore {
    async fn get(&self, name: &str) -> Result<Option<Id>> {
        let name = name.to_owned();
        // Autocommit reads in WAL mode see every transaction committed before they start.
        self.read(move |c| {
            c.prepare_cached("SELECT id FROM refs WHERE name = ?1")?
                .query_row([&name], |r| r.get::<_, Vec<u8>>(0))
                .optional()?
                .map(id_of)
                .transpose()
        })
        .await
    }

    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool> {
        let name = name.to_owned();
        self.write(move |c| {
            let changed = match expected {
                None => c
                    .prepare_cached("INSERT OR IGNORE INTO refs (name, id) VALUES (?1, ?2)")?
                    .execute(params![name, &new.0[..]])?,
                Some(old) => c
                    .prepare_cached("UPDATE refs SET id = ?1 WHERE name = ?2 AND id = ?3")?
                    .execute(params![&new.0[..], name, &old.0[..]])?,
            };
            Ok(changed == 1)
        })
        .await
    }

    async fn list(&self) -> Result<Vec<(String, Id)>> {
        self.read(|c| {
            c.prepare_cached("SELECT name, id FROM refs ORDER BY name")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, id_of(r.get(1)?)?)))?
                .collect()
        })
        .await
    }
}

#[async_trait]
impl DerivedStore for SqliteStore {
    async fn get(&self, kind: &str, id: &Id) -> Result<Option<Bytes>> {
        let (kind, id) = (kind.to_owned(), *id);
        self.read(move |c| {
            c.prepare_cached("SELECT value FROM derived WHERE kind = ?1 AND id = ?2")?
                .query_row(params![kind, &id.0[..]], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map(|o| o.map(Bytes::from))
        })
        .await
    }

    async fn put(&self, kind: &str, id: Id, value: Bytes) -> Result<()> {
        let kind = kind.to_owned();
        self.write(move |c| {
            c.prepare_cached("INSERT OR REPLACE INTO derived (kind, id, value) VALUES (?1, ?2, ?3)")?
                .execute(params![kind, &id.0[..], &value[..]])
                .map(drop)
        })
        .await
    }
}
