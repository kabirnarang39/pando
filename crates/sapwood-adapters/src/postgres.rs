//! PostgreSQL adapter for highly-available, multi-server deployments. Several stateless
//! `sapwood server` replicas can share one database: ref moves are single-row
//! conditional updates, so replicas need no coordination beyond Postgres itself.

use async_trait::async_trait;
use bytes::Bytes;
use sapwood_core::ports::{ChangesetLog, DerivedStore, ObjectStore, RefStore};
use sapwood_core::{Id, Result};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::storage_err;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sapwood_objects    (id BYTEA PRIMARY KEY, payload BYTEA NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sapwood_changesets (id BYTEA PRIMARY KEY, data BYTEA NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sapwood_refs       (name TEXT PRIMARY KEY, id BYTEA NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sapwood_derived    (kind TEXT NOT NULL, id BYTEA NOT NULL, value BYTEA NOT NULL, PRIMARY KEY (kind, id))",
];

#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

fn to_id(b: Vec<u8>) -> Result<Id> {
    Id::from_slice(&b)
}

impl PostgresStore {
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(max_connections).connect(url).await.map_err(storage_err)?;
        for stmt in SCHEMA {
            sqlx::query(*stmt).execute(&pool).await.map_err(storage_err)?;
        }
        Ok(Self { pool })
    }
}

#[async_trait]
impl ObjectStore for PostgresStore {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        let row = sqlx::query("SELECT payload FROM sapwood_objects WHERE id = $1")
            .bind(&id.0[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_err)?;
        Ok(row.map(|r| Bytes::from(r.get::<Vec<u8>, _>(0))))
    }

    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        self.put_many(vec![(id, payload)]).await
    }

    /// One round trip for the whole batch via UNNEST.
    async fn put_many(&self, items: Vec<(Id, Bytes)>) -> Result<()> {
        let (ids, payloads): (Vec<Vec<u8>>, Vec<Vec<u8>>) = items.into_iter().map(|(i, p)| (i.0.to_vec(), p.to_vec())).unzip();
        sqlx::query(
            "INSERT INTO sapwood_objects (id, payload) SELECT * FROM UNNEST($1::bytea[], $2::bytea[]) ON CONFLICT DO NOTHING",
        )
        .bind(ids)
        .bind(payloads)
        .execute(&self.pool)
        .await
        .map_err(storage_err)?;
        Ok(())
    }

    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.missing(std::slice::from_ref(id)).await?.is_empty())
    }

    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        let keys: Vec<Vec<u8>> = ids.iter().map(|i| i.0.to_vec()).collect();
        let rows = sqlx::query("SELECT id, payload FROM sapwood_objects WHERE id = ANY($1)")
            .bind(keys)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_err)?;
        let mut found = std::collections::HashMap::with_capacity(rows.len());
        for r in rows {
            found.insert(to_id(r.get(0))?, Bytes::from(r.get::<Vec<u8>, _>(1)));
        }
        Ok(ids.iter().map(|i| found.get(i).cloned()).collect())
    }

    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let keys: Vec<Vec<u8>> = ids.iter().map(|i| i.0.to_vec()).collect();
        let rows = sqlx::query("SELECT id FROM sapwood_objects WHERE id = ANY($1)")
            .bind(keys)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_err)?;
        let present: std::collections::HashSet<Id> = rows.into_iter().map(|r| to_id(r.get(0))).collect::<Result<_>>()?;
        Ok(ids.iter().filter(|i| !present.contains(i)).copied().collect())
    }
}

#[async_trait]
impl ChangesetLog for PostgresStore {
    async fn append(&self, id: Id, encoded: Bytes) -> Result<()> {
        sqlx::query("INSERT INTO sapwood_changesets (id, data) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(&id.0[..])
            .bind(&encoded[..])
            .execute(&self.pool)
            .await
            .map_err(storage_err)?;
        Ok(())
    }

    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        let row = sqlx::query("SELECT data FROM sapwood_changesets WHERE id = $1")
            .bind(&id.0[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_err)?;
        Ok(row.map(|r| Bytes::from(r.get::<Vec<u8>, _>(0))))
    }
}

#[async_trait]
impl RefStore for PostgresStore {
    async fn get(&self, name: &str) -> Result<Option<Id>> {
        let row = sqlx::query("SELECT id FROM sapwood_refs WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_err)?;
        row.map(|r| to_id(r.get(0))).transpose()
    }

    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool> {
        let result = match expected {
            None => sqlx::query("INSERT INTO sapwood_refs (name, id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(name)
                .bind(&new.0[..])
                .execute(&self.pool)
                .await,
            Some(old) => sqlx::query("UPDATE sapwood_refs SET id = $1 WHERE name = $2 AND id = $3")
                .bind(&new.0[..])
                .bind(name)
                .bind(&old.0[..])
                .execute(&self.pool)
                .await,
        };
        Ok(result.map_err(storage_err)?.rows_affected() == 1)
    }

    async fn list(&self) -> Result<Vec<(String, Id)>> {
        sqlx::query("SELECT name, id FROM sapwood_refs ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_err)?
            .into_iter()
            .map(|r| Ok((r.get(0), to_id(r.get(1))?)))
            .collect()
    }
}

#[async_trait]
impl DerivedStore for PostgresStore {
    async fn get(&self, kind: &str, id: &Id) -> Result<Option<Bytes>> {
        let row = sqlx::query("SELECT value FROM sapwood_derived WHERE kind = $1 AND id = $2")
            .bind(kind)
            .bind(&id.0[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_err)?;
        Ok(row.map(|r| Bytes::from(r.get::<Vec<u8>, _>(0))))
    }

    async fn put(&self, kind: &str, id: Id, value: Bytes) -> Result<()> {
        sqlx::query(
            "INSERT INTO sapwood_derived (kind, id, value) VALUES ($1, $2, $3) ON CONFLICT (kind, id) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(kind)
        .bind(&id.0[..])
        .bind(&value[..])
        .execute(&self.pool)
        .await
        .map_err(storage_err)?;
        Ok(())
    }
}
