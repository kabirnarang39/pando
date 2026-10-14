//! gRPC upstream: an `ObjectStore` backed by another tier's `Objects` service. This is
//! what lets a node daemon sit on a regional cache, and a regional cache on the server,
//! with identical RPCs at every hop.
//!
//! Payloads received from upstream are hash-verified before being returned, so a tier
//! never caches (and forever re-serves) a corrupted object.

use async_trait::async_trait;
use bytes::Bytes;
use sapwood_core::object::decode_object;
use sapwood_core::ports::ObjectStore;
use sapwood_core::{Error, Id, Result};
use sapwood_proto::objects_client::ObjectsClient;
use sapwood_proto::{BearerAuth, IdList, MAX_MESSAGE_BYTES, OBJECT_BATCH, ObjectPayload};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

use crate::storage_err;

type Client = ObjectsClient<InterceptedService<Channel, BearerAuth>>;

#[derive(Clone)]
pub struct GrpcObjects {
    client: Client,
}

pub fn status_err(s: tonic::Status) -> Error {
    match s.code() {
        tonic::Code::NotFound => Error::NotFound(s.message().to_owned()),
        tonic::Code::InvalidArgument => Error::Invalid(s.message().to_owned()),
        _ => storage_err(format!("upstream: {s}")),
    }
}

impl GrpcObjects {
    /// Lazily connecting channel: the first request dials, and tonic reconnects on failure.
    pub fn connect(url: &str, token: Option<&str>) -> Result<Self> {
        let channel = Endpoint::from_shared(url.to_owned())
            .map_err(storage_err)?
            .tcp_nodelay(true)
            .http2_adaptive_window(true)
            .connect_lazy();
        Ok(Self::from_channel(channel, token)?)
    }

    pub fn from_channel(channel: Channel, token: Option<&str>) -> Result<Self> {
        let auth = BearerAuth::new(token).map_err(status_err)?;
        let client = ObjectsClient::with_interceptor(channel, auth)
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES);
        Ok(Self { client })
    }
}

#[async_trait]
impl ObjectStore for GrpcObjects {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        Ok(self.get_many(std::slice::from_ref(id)).await?.pop().flatten())
    }

    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        self.put_many(vec![(id, payload)]).await
    }

    async fn put_many(&self, items: Vec<(Id, Bytes)>) -> Result<()> {
        let msgs = items.into_iter().map(|(id, p)| ObjectPayload { id: id.0.to_vec(), payload: p.to_vec() });
        self.client.clone().put(tokio_stream::iter(msgs)).await.map_err(status_err)?;
        Ok(())
    }

    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.missing(std::slice::from_ref(id)).await?.is_empty())
    }

    async fn get_many(&self, ids: &[Id]) -> Result<Vec<Option<Bytes>>> {
        let mut found = std::collections::HashMap::with_capacity(ids.len());
        for chunk in ids.chunks(OBJECT_BATCH) {
            let req = IdList { ids: chunk.iter().map(|i| i.0.to_vec()).collect() };
            let mut stream = self.client.clone().get(req).await.map_err(status_err)?.into_inner();
            while let Some(msg) = stream.message().await.map_err(status_err)? {
                let id = Id::from_slice(&msg.id)?;
                // Verify on a blocking thread: decompress + hash is CPU work.
                let payload = Bytes::from(msg.payload);
                let check = payload.clone();
                tokio::task::spawn_blocking(move || decode_object(&id, &check)).await.map_err(storage_err)??;
                found.insert(id, payload);
            }
        }
        Ok(ids.iter().map(|i| found.get(i).cloned()).collect())
    }

    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(OBJECT_BATCH) {
            let req = IdList { ids: chunk.iter().map(|i| i.0.to_vec()).collect() };
            let resp = self.client.clone().missing(req).await.map_err(status_err)?.into_inner();
            for raw in resp.ids {
                out.push(Id::from_slice(&raw)?);
            }
        }
        Ok(out)
    }
}
