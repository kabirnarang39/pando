//! Domain error -> gRPC status. The single place transport semantics are decided.

use sapwood_core::{Error, Id};
use futures::Stream;
use tonic::Status;

pub fn to_status(e: Error) -> Status {
    match e {
        Error::NotFound(m) => Status::not_found(m),
        Error::Conflict(m) => Status::aborted(m),
        Error::Invalid(m) => Status::invalid_argument(m),
        Error::Corrupt(m) => Status::data_loss(m),
        Error::Disabled(f) => Status::unimplemented(format!("feature `{f}` is disabled on this server")),
        Error::Storage(e) => {
            tracing::error!(error = %e, "storage failure");
            Status::internal("internal storage error")
        }
    }
}

pub fn id_from(bytes: &[u8]) -> Result<Id, Status> {
    Id::from_slice(bytes).map_err(to_status)
}

pub type RpcResult<T> = Result<tonic::Response<T>, Status>;

/// Bounded channel-backed stream: applies backpressure to storage reads when the client
/// reads slowly (at most 32 payloads buffered per request).
pub fn async_stream<T, F, Fut>(f: F) -> impl Stream<Item = Result<T, Status>> + Send
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<Result<T, Status>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(f(tx));
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// Wire view of a changeset, including its derived root tree and generation.
pub async fn changeset_info(repo: &sapwood_core::Repo, id: Id) -> sapwood_core::Result<sapwood_proto::ChangesetInfo> {
    let encoded = repo.log.get(&id).await?.ok_or_else(|| Error::not_found(format!("changeset {id}")))?;
    let (root, graph) = futures::try_join!(repo.root_tree(id), repo.graph(id))?;
    Ok(sapwood_proto::ChangesetInfo {
        id: id.0.to_vec(),
        root_tree: root.0.to_vec(),
        generation: graph.generation,
        encoded: encoded.to_vec(),
    })
}
