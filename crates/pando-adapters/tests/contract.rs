//! Port contract tests. Every adapter must pass the same suite, which is what makes
//! adapters substitutable (Liskov) rather than merely type-compatible.
//!
//! Postgres runs when `PANDO_TEST_POSTGRES_URL` is set (CI provides a service container).

use std::sync::Arc;

use bytes::Bytes;
use pando_adapters::cache::{BoundedCache, CachedDerived, CachedLog, Tiered};
use pando_adapters::cloud::CloudObjects;
use pando_adapters::memory::MemoryStore;
use pando_adapters::sqlite::SqliteStore;
use pando_core::Id;
use pando_core::object::{Kind, encode_object};
use pando_core::ports::{ChangesetLog, DerivedStore, ObjectStore, RefStore};

fn obj(n: u32) -> (Id, Bytes) {
    encode_object(Kind::Blob, format!("object-{n}").as_bytes())
}

async fn object_contract(s: &dyn ObjectStore) {
    let (a, pa) = obj(1);
    let (b, pb) = obj(2);
    let (c, _) = obj(3);
    assert_eq!(s.get(&a).await.unwrap(), None);
    assert!(!s.has(&a).await.unwrap());
    s.put(a, pa.clone()).await.unwrap();
    s.put(a, pa.clone()).await.unwrap(); // idempotent
    s.put_many(vec![(b, pb.clone())]).await.unwrap();
    s.put_many(Vec::new()).await.unwrap();
    assert_eq!(s.get(&a).await.unwrap(), Some(pa.clone()));
    assert!(s.has(&b).await.unwrap());
    assert_eq!(s.get_many(&[c, a, b, a]).await.unwrap(), vec![None, Some(pa.clone()), Some(pb), Some(pa.clone())]);
    assert_eq!(s.missing(&[a, c, b]).await.unwrap(), vec![c]);
    assert_eq!(s.missing(&[]).await.unwrap(), Vec::<Id>::new());

    // replace: in-place representation change of an existing object, or `false` when the
    // backend does not support it. Never creates objects.
    let alt = Bytes::from_static(b"alternate-representation");
    if s.replace(a, alt.clone()).await.unwrap() {
        assert_eq!(s.get(&a).await.unwrap(), Some(alt));
        assert!(s.replace(a, pa.clone()).await.unwrap());
    }
    assert!(!s.replace(c, pa.clone()).await.unwrap(), "replace must not create");
    assert_eq!(s.get(&c).await.unwrap(), None);
    assert_eq!(s.get(&a).await.unwrap(), Some(pa.clone()));

    // Bulk: many objects in one batch round-trip intact.
    let many: Vec<(Id, Bytes)> = (100..1100).map(obj).collect();
    s.put_many(many.clone()).await.unwrap();
    let ids: Vec<Id> = many.iter().map(|(i, _)| *i).collect();
    let got = s.get_many(&ids).await.unwrap();
    assert!(got.iter().zip(&many).all(|(g, (_, p))| g.as_ref() == Some(p)));
}

async fn log_contract(s: &dyn ChangesetLog) {
    let id = Id::hash(3, b"cs");
    assert_eq!(s.get(&id).await.unwrap(), None);
    s.append(id, Bytes::from_static(b"one")).await.unwrap();
    s.append(id, Bytes::from_static(b"two")).await.unwrap(); // first write wins
    assert_eq!(s.get(&id).await.unwrap(), Some(Bytes::from_static(b"one")));
}

async fn ref_contract(s: Arc<dyn RefStore>) {
    let (x, y, z) = (Id([1; 32]), Id([2; 32]), Id([3; 32]));
    assert_eq!(s.get("main").await.unwrap(), None);
    assert!(!s.compare_and_swap("main", Some(x), y).await.unwrap(), "CAS on absent ref with expectation");
    assert!(s.compare_and_swap("main", None, x).await.unwrap());
    assert!(!s.compare_and_swap("main", None, y).await.unwrap(), "create over existing");
    assert!(!s.compare_and_swap("main", Some(z), y).await.unwrap(), "stale expectation");
    assert!(s.compare_and_swap("main", Some(x), y).await.unwrap());
    assert_eq!(s.get("main").await.unwrap(), Some(y));
    assert!(s.compare_and_swap("release/1", None, z).await.unwrap());
    assert_eq!(s.list().await.unwrap(), vec![("main".into(), y), ("release/1".into(), z)]);

    // Linearizability under contention: 16 writers x 25 increments through CAS retry
    // loops must produce exactly 400 successful swaps forming one chain.
    let counter = |n: u64| {
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&n.to_le_bytes());
        Id(b)
    };
    assert!(s.compare_and_swap("ctr", None, counter(0)).await.unwrap());
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let s = s.clone();
            tokio::spawn(async move {
                for _ in 0..25 {
                    loop {
                        let cur = s.get("ctr").await.unwrap().unwrap();
                        let n = u64::from_le_bytes(cur.0[..8].try_into().unwrap());
                        if s.compare_and_swap("ctr", Some(cur), counter(n + 1)).await.unwrap() {
                            break;
                        }
                    }
                }
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(s.get("ctr").await.unwrap(), Some(counter(400)));
}

async fn derived_contract(s: &dyn DerivedStore) {
    let id = Id([9; 32]);
    assert_eq!(s.get("k.v1", &id).await.unwrap(), None);
    s.put("k.v1", id, Bytes::from_static(b"a")).await.unwrap();
    assert_eq!(s.get("k.v1", &id).await.unwrap(), Some(Bytes::from_static(b"a")));
    assert_eq!(s.get("k.v2", &id).await.unwrap(), None, "kinds are namespaced");
    s.put("k.v1", id, Bytes::from_static(b"b")).await.unwrap(); // recompute overwrites
    assert_eq!(s.get("k.v1", &id).await.unwrap(), Some(Bytes::from_static(b"b")));
}

async fn full_contract<S>(s: Arc<S>)
where
    S: ObjectStore + ChangesetLog + RefStore + DerivedStore + 'static,
{
    object_contract(s.as_ref()).await;
    log_contract(s.as_ref()).await;
    ref_contract(s.clone()).await;
    derived_contract(s.as_ref()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_store() {
    full_contract(Arc::new(MemoryStore::new())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_store() {
    let dir = tempfile::tempdir().unwrap();
    full_contract(Arc::new(SqliteStore::open(dir.path().join("db.sqlite")).unwrap())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    let (a, pa) = obj(1);
    {
        let s = SqliteStore::open(&path).unwrap();
        ObjectStore::put(&s, a, pa.clone()).await.unwrap();
        assert!(s.compare_and_swap("main", None, a).await.unwrap());
    }
    let s = SqliteStore::open(&path).unwrap();
    assert_eq!(ObjectStore::get(&s, &a).await.unwrap(), Some(pa));
    assert_eq!(RefStore::get(&s, "main").await.unwrap(), Some(a));
}

#[tokio::test(flavor = "multi_thread")]
async fn object_storage_local_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let url = pando_adapters::cloud::file_url(&dir.path().join("objs")).unwrap();
    object_contract(&CloudObjects::from_url(&url).unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn object_storage_memory() {
    object_contract(&CloudObjects::from_url("memory:///").unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bounded_cache() {
    object_contract(&BoundedCache::new(64 << 20)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tiered_read_through_write_through() {
    let upstream = Arc::new(MemoryStore::new());
    let local = Arc::new(BoundedCache::new(64 << 20));
    let tier = Tiered::new(local.clone(), upstream.clone());
    object_contract(&tier).await;

    // Writes reach the authority (write-around: not the local tier).
    let (a, pa) = obj(1);
    assert_eq!(ObjectStore::get(upstream.as_ref(), &a).await.unwrap(), Some(pa));

    // Read-through fills the local tier; second read is a hit.
    let (d, pd) = obj(77_777);
    ObjectStore::put(upstream.as_ref(), d, pd.clone()).await.unwrap();
    assert!(!local.has(&d).await.unwrap());
    assert_eq!(tier.get(&d).await.unwrap(), Some(pd));
    assert!(local.has(&d).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn bounded_cache_respects_budget() {
    let cache = BoundedCache::new(1 << 20);
    for n in 0..10_000 {
        let (id, p) = encode_object(Kind::Blob, &vec![n as u8; 1024 + n as usize % 7]);
        cache.put(id, p).await.unwrap();
    }
    assert!(cache.weight() <= 1 << 20, "weight {} over budget", cache.weight());
}

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
async fn postgres_store() {
    let Ok(url) = std::env::var("PANDO_TEST_POSTGRES_URL") else {
        eprintln!("skipping: PANDO_TEST_POSTGRES_URL not set");
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    // A database from before the rename (tables named `sapwood_*`) keeps its data.
    for t in ["objects", "changesets", "refs", "derived"] {
        for prefix in ["pando", "sapwood"] {
            let drop = format!("DROP TABLE IF EXISTS {prefix}_{t}");
            sqlx::query(sqlx::AssertSqlSafe(drop)).execute(&pool).await.unwrap();
        }
    }
    sqlx::query("CREATE TABLE sapwood_refs (name TEXT PRIMARY KEY, id BYTEA NOT NULL)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO sapwood_refs VALUES ('main', $1)").bind(vec![7u8; 32]).execute(&pool).await.unwrap();
    let s = pando_adapters::postgres::PostgresStore::connect(&url, 16).await.unwrap();
    assert_eq!(RefStore::get(&s, "main").await.unwrap(), Some(Id([7; 32])));
    // Isolate from previous runs.
    sqlx::query("TRUNCATE pando_objects, pando_changesets, pando_refs, pando_derived").execute(&pool).await.unwrap();
    full_contract(Arc::new(s)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn object_storage_s3() {
    let Ok(url) = std::env::var("PANDO_TEST_S3_URL") else {
        eprintln!("skipping: PANDO_TEST_S3_URL not set");
        return;
    };
    object_contract(&CloudObjects::from_url(&url).unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_metadata_decorators() {
    let inner = Arc::new(MemoryStore::new());
    let log = CachedLog::new(inner.clone(), 1 << 20);
    log_contract(&log).await;
    let derived = CachedDerived::new(inner.clone(), 1 << 20);
    derived_contract(&derived).await;
    // Writes through the decorator reach the backing store.
    let id = Id::hash(3, b"cs");
    assert!(ChangesetLog::get(inner.as_ref(), &id).await.unwrap().is_some());
    assert!(DerivedStore::get(inner.as_ref(), "k.v1", &Id([9; 32])).await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn delta_reading_decorator() {
    use pando_adapters::delta::DeltaObjects;
    use pando_core::delta;

    let inner = Arc::new(MemoryStore::new());
    let d = DeltaObjects::new(inner.clone());
    object_contract(&d).await;

    // A two-level delta chain reads back as ordinary, verified payloads.
    let v1 = b"line one\nline two\nline three\n".repeat(200);
    let mut v2 = v1.clone();
    v2.extend_from_slice(b"added in v2\n");
    let mut v3 = v2.clone();
    v3.extend_from_slice(b"added in v3\n");
    let (i1, p1) = encode_object(Kind::Blob, &v1);
    let (i2, p2) = encode_object(Kind::Blob, &v2);
    let (i3, p3) = encode_object(Kind::Blob, &v3);
    ObjectStore::put_many(&d, vec![(i1, p1.clone()), (i2, p2.clone()), (i3, p3.clone())]).await.unwrap();
    assert!(d.replace(i2, delta::encode(i1, 1, &v1, &v2).unwrap().into()).await.unwrap());
    assert!(d.replace(i3, delta::encode(i2, 2, &v2, &v3).unwrap().into()).await.unwrap());
    assert!(delta::is_delta(&ObjectStore::get(inner.as_ref(), &i3).await.unwrap().unwrap()));
    let got = d.get_many(&[i3, i1, i2]).await.unwrap();
    let decoded: Vec<Vec<u8>> = got
        .iter()
        .zip([i3, i1, i2])
        .map(|(p, id)| pando_core::object::decode_object(&id, p.as_ref().unwrap()).unwrap().1)
        .collect();
    assert_eq!(decoded, vec![v3.clone(), v1.clone(), v2.clone()]);

    // Corruption is detected, never served: point i3's delta at the wrong base.
    assert!(d.replace(i3, delta::encode(i1, 1, &v1, &v3).unwrap().into()).await.unwrap());
    assert!(d.get(&i3).await.is_ok(), "valid alternate base still reconstructs v3");
    assert!(d.replace(i3, delta::encode(i2, 1, &v1, &v3).unwrap().into()).await.unwrap());
    assert!(d.get(&i3).await.is_err(), "frame built against v1 but labelled base v2 must fail verification");
    // Cycles are detected rather than looping forever.
    assert!(d.replace(i1, delta::encode(i3, 1, &v3, &v1).unwrap().into()).await.unwrap());
    assert!(d.replace(i3, delta::encode(i1, 1, &v1, &v3).unwrap().into()).await.unwrap());
    let err = d.get(&i3).await.unwrap_err().to_string();
    assert!(err.contains("cycle"), "{err}");
}
