//! Objects slice: content-addressed get/put/missing. Served identically by every tier.

use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use pando_core::acl::{AccessPolicy, Principal, effective_scope, require_full_read};
use pando_core::chunked::{is_manifest, verify_manifest};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{MAX_OBJECT_BYTES, decode_object};
use pando_core::ports::ObjectStore;
use pando_core::{Error, Id, Result};
use pando_proto::objects_server::{self, ObjectsServer};
use pando_proto::{IdList, MAX_MESSAGE_BYTES, OBJECT_BATCH, ObjectPayload, PutObjectsResponse};
use tonic::{Request, Response, Status, Streaming};

use crate::auth::principal;
use crate::status::{RpcResult, async_stream, id_from, to_status};

#[derive(Clone)]
pub struct Objects {
    store: Arc<dyn ObjectStore>,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Objects {
    pub fn new(store: Arc<dyn ObjectStore>, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { store, flags, policy }
    }

    /// Reading objects by id bypasses path checks, so it requires full read access.
    /// Writing (Put) is always allowed: it reveals nothing.
    fn require_object_reads(&self, who: &Principal) -> Result<()> {
        require_full_read(&effective_scope(self.flags.as_ref(), self.policy.as_ref(), who), who)
    }

    pub async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        require(self.flags.as_ref(), Flag::Objects)?;
        self.store.get_many(ids).await
    }

    pub async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        require(self.flags.as_ref(), Flag::Objects)?;
        self.store.missing(ids).await
    }

    /// Every uploaded payload is verified (decompress + hash) before storage, on the
    /// blocking pool, so no tier ever stores an object that does not match its id.
    pub async fn put_many(&self, items: Vec<(Id, Bytes)>) -> Result<()> {
        require(self.flags.as_ref(), Flag::Objects)?;
        if items.is_empty() {
            return Ok(());
        }
        // Large-file manifests are verified after the ordinary objects of the same batch are
        // stored, by streaming their chunks and hashing the reassembled file against its id.
        let (manifests, items): (Vec<_>, Vec<_>) = items.into_iter().partition(|(_, p)| is_manifest(p));
        let items = tokio::task::spawn_blocking(move || {
            for (id, payload) in &items {
                // Compressed size can only exceed the bound for incompressible junk.
                if payload.len() as u64 > MAX_OBJECT_BYTES + 1024 {
                    return Err(Error::invalid(format!("object {id} exceeds size limit")));
                }
                decode_object(id, payload)?;
            }
            Ok(items)
        })
        .await
        .map_err(|e| Error::Storage(e.into()))??;
        self.store.put_many(items).await?;
        for (id, payload) in &manifests {
            verify_manifest(self.store.as_ref(), id, payload).await?;
        }
        self.store.put_many(manifests).await
    }
}

/// Upper bound on ids per request, bounding per-request memory.
const MAX_IDS_PER_REQUEST: usize = 64 * OBJECT_BATCH;

pub fn service(objects: Objects) -> ObjectsServer<Objects> {
    ObjectsServer::new(objects)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES)
}

fn ids(list: &IdList) -> Result<Vec<pando_core::Id>, Status> {
    if list.ids.len() > MAX_IDS_PER_REQUEST {
        return Err(Status::invalid_argument(format!("at most {MAX_IDS_PER_REQUEST} ids per request")));
    }
    list.ids.iter().map(|b| id_from(b)).collect()
}

#[tonic::async_trait]
impl objects_server::Objects for Objects {
    type GetStream = Pin<Box<dyn Stream<Item = Result<ObjectPayload, Status>> + Send>>;

    /// Streams found objects in request order, fetching from storage in batches so the
    /// first bytes leave before the whole request is resolved. Missing ids are omitted.
    async fn get(&self, req: Request<IdList>) -> RpcResult<Self::GetStream> {
        self.require_object_reads(&principal(&req)).map_err(to_status)?;
        let ids = ids(req.get_ref())?;
        let this = self.clone();
        let stream = async_stream(move |tx| async move {
            for chunk in ids.chunks(OBJECT_BATCH) {
                let found = match this.get_many(chunk).await {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = tx.send(Err(to_status(e))).await;
                        return;
                    }
                };
                for (id, payload) in chunk.iter().zip(found) {
                    if let Some(p) = payload
                        && tx.send(Ok(ObjectPayload { id: id.0.to_vec(), payload: p.to_vec() })).await.is_err()
                    {
                        return; // client went away
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(stream)))
    }

    async fn put(&self, req: Request<Streaming<ObjectPayload>>) -> RpcResult<PutObjectsResponse> {
        let mut stream = req.into_inner();
        let mut batch = Vec::new();
        let mut batch_bytes = 0usize;
        let mut stored = 0u32;
        while let Some(msg) = stream.message().await? {
            batch_bytes += msg.payload.len();
            batch.push((id_from(&msg.id)?, bytes::Bytes::from(msg.payload)));
            // Flush in bounded batches: one storage transaction per ~64 MiB / 1024 objects.
            if batch.len() >= OBJECT_BATCH || batch_bytes >= 64 << 20 {
                stored += batch.len() as u32;
                self.put_many(std::mem::take(&mut batch)).await.map_err(to_status)?;
                batch_bytes = 0;
            }
        }
        stored += batch.len() as u32;
        self.put_many(batch).await.map_err(to_status)?;
        Ok(Response::new(PutObjectsResponse { stored }))
    }

    async fn missing(&self, req: Request<IdList>) -> RpcResult<IdList> {
        self.require_object_reads(&principal(&req)).map_err(to_status)?;
        let missing = Objects::missing(self, &ids(req.get_ref())?).await.map_err(to_status)?;
        Ok(Response::new(IdList { ids: missing.into_iter().map(|i| i.0.to_vec()).collect() }))
    }
}
