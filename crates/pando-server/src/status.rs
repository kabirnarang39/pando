//! Domain error -> gRPC status. The single place transport semantics are decided.

use futures::Stream;
use pando_core::{Error, Id};
use tonic::Status;

pub fn to_status(e: Error) -> Status {
    match e {
        Error::NotFound(m) => Status::not_found(m),
        Error::Conflict(m) => Status::aborted(m),
        Error::Invalid(m) => Status::invalid_argument(m),
        Error::Corrupt(m) => Status::data_loss(m),
        Error::PermissionDenied(m) => Status::permission_denied(m),
        Error::Rejected(m) => Status::failed_precondition(m),
        Error::Disabled(f) => Status::unimplemented(format!("feature `{f}` is disabled on this server")),
        Error::Storage(e) => internal(e),
    }
}

/// An internal failure: the details go to the log under a random id, the client gets only the
/// id, so storage errors, SQL and paths never reach callers.
pub fn internal(e: impl std::fmt::Display) -> Status {
    let mut raw = [0u8; 6];
    let _ = getrandom::fill(&mut raw);
    let id = hex::encode(raw);
    tracing::error!(error = %e, error_id = %id, "internal error");
    Status::internal(format!("internal error (id {id}); the server log has the details"))
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
/// Changes outside `scope` are removed (and the result marked `redacted`).
pub async fn changeset_info(
    repo: &pando_core::Repo,
    id: Id,
    scope: &pando_core::acl::ReadScope,
) -> pando_core::Result<pando_proto::ChangesetInfo> {
    let encoded = repo.log.get(&id).await?.ok_or_else(|| Error::not_found(format!("changeset {id}")))?;
    let (root, graph) = futures::try_join!(repo.root_tree(id), repo.graph(id))?;
    let (encoded, redacted) = if scope.is_full() {
        (encoded.to_vec(), false)
    } else {
        let cs = pando_core::object::Changeset::decode_verified(&id, &encoded)?;
        match pando_core::acl::redact(&cs, scope) {
            (visible, true) => (visible.encode_canonical().1.to_vec(), true),
            (_, false) => (encoded.to_vec(), false),
        }
    };
    Ok(pando_proto::ChangesetInfo {
        id: id.0.to_vec(),
        root_tree: root.0.to_vec(),
        generation: graph.generation,
        encoded,
        redacted,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn internal_errors_reveal_only_an_id() {
        let s = super::internal("no such table: secret_things at /var/lib/pando/platform.db");
        assert_eq!(s.code(), tonic::Code::Internal);
        assert!(s.message().starts_with("internal error (id ") && !s.message().contains("secret"), "{}", s.message());
    }
}
