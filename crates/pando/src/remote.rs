//! Remote port: everything the client needs from the server, behind one trait so the
//! transport (gRPC today; HTTP/JSON, in-process, a mock) is replaceable.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use pando_adapters::grpc::{GrpcObjects, status_err};
use pando_core::acl::ReadScope;
use pando_core::object::{Changeset, FileChange, Mode};
use pando_core::path::components as validate_path;
use pando_core::ports::ObjectStore;
use pando_core::{Id, path::validate_name};
use pando_proto::access_client::AccessClient;
use pando_proto::browse_client::BrowseClient;
use pando_proto::changesets_client::ChangesetsClient;
use pando_proto::diff_client::DiffClient;
use pando_proto::history_client::HistoryClient;
use pando_proto::land_client::LandClient;
use pando_proto::locks_client::LocksClient;
use pando_proto::merge_base_client::MergeBaseClient;
use pando_proto::refs_client::RefsClient;
use pando_proto::{
    BearerAuth, BlameRequest, BrowseRequest, CasRequest, CheckoutRequest, CreateChangesetRequest, DiffRequest, Empty,
    FindRequest, LandRequest, ListLocksRequest, LockRequest, LogRequest, MAX_MESSAGE_BYTES, MergeBaseRequest, RefName,
    ResolveRequest, UnlockRequest,
};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

/// A changeset as seen by the client: verified content plus server-derived data.
#[derive(Debug, Clone)]
pub struct RemoteChangeset {
    pub id: Id,
    /// Changes outside the caller's read scope were removed by the server.
    pub redacted: bool,
    pub root_tree: Id,
    pub generation: u64,
    pub changeset: Changeset,
}

/// A piece of a file from a path-scoped checkout; `last` ends the file.
#[derive(Debug, Clone)]
pub struct Piece {
    pub path: String,
    pub mode: Mode,
    pub id: Id,
    pub data: Vec<u8>,
    pub last: bool,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub mode: Mode,
    /// Absent for directories the caller may traverse but not read.
    pub id: Option<Id>,
}

#[async_trait]
pub trait Remote: Send + Sync {
    /// Object access; may point at a node daemon or regional cache instead of the server.
    fn objects(&self) -> Arc<dyn ObjectStore>;
    async fn create_changeset(&self, cs: Changeset) -> Result<RemoteChangeset>;
    async fn changeset(&self, rev: &str) -> Result<RemoteChangeset>;
    async fn get_ref(&self, name: &str) -> Result<Option<Id>>;
    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool>;
    async fn list_refs(&self) -> Result<Vec<(String, Id)>>;
    async fn land(&self, ref_name: &str, cs: Id) -> Result<(Id, bool)>;
    async fn log(&self, rev: &str, path: &str, limit: u32) -> Result<Vec<RemoteChangeset>>;
    async fn diff(&self, from: &str, to: &str) -> Result<Vec<FileChange>>;
    async fn merge_base(&self, a: &str, b: &str) -> Result<Option<Id>>;
    async fn list_dir(&self, rev: &str, path: &str) -> Result<Vec<DirEntry>>;
    async fn read_file(&self, rev: &str, path: &str) -> Result<(Mode, Vec<u8>)>;
    /// The caller's read scope.
    async fn whoami(&self) -> Result<ReadScope>;
    /// Stream readable files under `paths` (everything readable when empty) at `rev`.
    async fn checkout(
        &self,
        rev: &str,
        paths: &[String],
        sink: &mut (dyn FnMut(Piece) -> Result<()> + Send),
    ) -> Result<()>;
    async fn find(&self, rev: &str, pattern: &str, limit: u32) -> Result<Vec<String>>;
    /// Lock a file for exclusive editing; returns (path, owner, locked at ms).
    async fn lock(&self, path: &str) -> Result<(String, String, i64)>;
    async fn unlock(&self, path: &str, force: bool) -> Result<()>;
    /// Locks under `prefix` ("" for all).
    async fn locks(&self, prefix: &str) -> Result<Vec<(String, String, i64)>>;
    /// (changeset that last changed the line, line text) for each line.
    async fn blame(&self, rev: &str, path: &str) -> Result<Vec<(Id, String)>>;
}

type Ch = InterceptedService<Channel, BearerAuth>;

#[derive(Clone)]
pub struct GrpcRemote {
    channel: Channel,
    auth: BearerAuth,
    objects: Arc<dyn ObjectStore>,
}

fn verify(info: pando_proto::ChangesetInfo) -> Result<RemoteChangeset> {
    let id = Id::from_slice(&info.id)?;
    // A redacted changeset (changes outside our read scope removed) cannot hash to its id;
    // everything else is verified.
    let changeset = if info.redacted {
        <Changeset as prost::Message>::decode(&info.encoded[..])?
    } else {
        Changeset::decode_verified(&id, &info.encoded)?
    };
    Ok(RemoteChangeset {
        changeset,
        redacted: info.redacted,
        root_tree: Id::from_slice(&info.root_tree)?,
        generation: info.generation,
        id,
    })
}

fn wire_change(c: FileChange) -> pando_proto::FileChange {
    pando_proto::FileChange { path: c.path, blob: c.blob, mode: c.mode }
}

impl GrpcRemote {
    /// `server` hosts all services; `objects_url` (daemon or cache) serves object reads,
    /// defaulting to the server.
    /// `server` may end in `/OWNER/NAME` to name a repository on a server that hosts many.
    pub fn connect(server: &str, objects_url: Option<&str>, token: Option<&str>) -> Result<Self> {
        let (base, repo) = pando_proto::split_repo_url(server);
        let channel = Endpoint::from_shared(base)
            .with_context(|| format!("bad server URL `{server}`"))?
            .tcp_nodelay(true)
            .http2_adaptive_window(true)
            .connect_lazy();
        let auth =
            BearerAuth::new(token).and_then(|a| a.with_repo(repo.as_deref())).map_err(|s| anyhow::anyhow!("{s}"))?;
        let objects: Arc<dyn ObjectStore> = match objects_url {
            Some(url) => Arc::new(GrpcObjects::connect(url, token)?),
            None => Arc::new(GrpcObjects::with_auth(channel.clone(), auth.clone())),
        };
        Ok(Self { channel, auth, objects })
    }

    fn ch(&self) -> Ch {
        InterceptedService::new(self.channel.clone(), self.auth.clone())
    }

    /// The hosting API: repositories and organisations (servers started with `--platform`).
    pub fn platform(&self) -> pando_proto::platform_client::PlatformClient<Ch> {
        pando_proto::platform_client::PlatformClient::new(self.ch())
    }
}

fn st(s: tonic::Status) -> anyhow::Error {
    status_err(s).into()
}

#[async_trait]
impl Remote for GrpcRemote {
    fn objects(&self) -> Arc<dyn ObjectStore> {
        self.objects.clone()
    }

    async fn create_changeset(&self, cs: Changeset) -> Result<RemoteChangeset> {
        let req = CreateChangesetRequest {
            parents: cs.parents,
            changes: cs.changes.into_iter().map(wire_change).collect(),
            author: cs.author,
            message: cs.message,
            timestamp_ms: cs.timestamp_ms,
        };
        let mut c = ChangesetsClient::new(self.ch()).max_encoding_message_size(MAX_MESSAGE_BYTES);
        verify(c.create(req).await.map_err(st)?.into_inner())
    }

    async fn changeset(&self, rev: &str) -> Result<RemoteChangeset> {
        let r = ChangesetsClient::new(self.ch()).get(ResolveRequest { rev: rev.to_owned() }).await.map_err(st)?;
        verify(r.into_inner())
    }

    async fn get_ref(&self, name: &str) -> Result<Option<Id>> {
        let r = RefsClient::new(self.ch()).get(RefName { name: name.to_owned() }).await.map_err(st)?.into_inner();
        if r.exists { Ok(Some(Id::from_slice(&r.id)?)) } else { Ok(None) }
    }

    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool> {
        let req = CasRequest {
            name: name.to_owned(),
            expect_exists: expected.is_some(),
            expected: expected.map(|i| i.0.to_vec()).unwrap_or_default(),
            new: new.0.to_vec(),
        };
        Ok(RefsClient::new(self.ch()).compare_and_swap(req).await.map_err(st)?.into_inner().swapped)
    }

    async fn list_refs(&self) -> Result<Vec<(String, Id)>> {
        let r = RefsClient::new(self.ch()).list(Empty {}).await.map_err(st)?.into_inner();
        r.refs.into_iter().map(|e| Ok((e.name, Id::from_slice(&e.id)?))).collect()
    }

    async fn land(&self, ref_name: &str, cs: Id) -> Result<(Id, bool)> {
        let req = LandRequest { r#ref: ref_name.to_owned(), changeset: cs.0.to_vec() };
        let r = LandClient::new(self.ch()).land(req).await.map_err(st)?.into_inner();
        Ok((Id::from_slice(&r.landed)?, r.rebased))
    }

    async fn log(&self, rev: &str, path: &str, limit: u32) -> Result<Vec<RemoteChangeset>> {
        let req = LogRequest { rev: rev.to_owned(), path: path.to_owned(), limit };
        let mut stream = HistoryClient::new(self.ch()).log(req).await.map_err(st)?.into_inner();
        let mut out = Vec::new();
        while let Some(info) = stream.message().await.map_err(st)? {
            out.push(verify(info)?);
        }
        Ok(out)
    }

    async fn diff(&self, from: &str, to: &str) -> Result<Vec<FileChange>> {
        let mut c = DiffClient::new(self.ch()).max_decoding_message_size(MAX_MESSAGE_BYTES);
        let r = c.diff(DiffRequest { from: from.to_owned(), to: to.to_owned() }).await.map_err(st)?.into_inner();
        Ok(r.changes.into_iter().map(|c| FileChange { path: c.path, blob: c.blob, mode: c.mode }).collect())
    }

    async fn merge_base(&self, a: &str, b: &str) -> Result<Option<Id>> {
        let req = MergeBaseRequest { a: a.to_owned(), b: b.to_owned() };
        let r = MergeBaseClient::new(self.ch()).merge_base(req).await.map_err(st)?.into_inner();
        if r.found { Ok(Some(Id::from_slice(&r.id)?)) } else { Ok(None) }
    }

    async fn list_dir(&self, rev: &str, path: &str) -> Result<Vec<DirEntry>> {
        let req = BrowseRequest { rev: rev.to_owned(), path: path.to_owned() };
        let r = BrowseClient::new(self.ch()).list_dir(req).await.map_err(st)?.into_inner();
        r.entries
            .into_iter()
            .map(|e| {
                validate_name(&e.name)?;
                Ok(DirEntry {
                    name: e.name,
                    mode: Mode::parse(e.mode)?,
                    id: if e.id.is_empty() { None } else { Some(Id::from_slice(&e.id)?) },
                })
            })
            .collect()
    }

    async fn read_file(&self, rev: &str, path: &str) -> Result<(Mode, Vec<u8>)> {
        let req = BrowseRequest { rev: rev.to_owned(), path: path.to_owned() };
        let mut c = BrowseClient::new(self.ch()).max_decoding_message_size(MAX_MESSAGE_BYTES);
        let r = c.read_file(req).await.map_err(st)?.into_inner();
        let id = Id::from_slice(&r.id)?;
        if pando_core::Id::hash(pando_core::object::Kind::Blob as u8, &r.content) != id {
            bail!("server returned content that does not match {id}");
        }
        Ok((Mode::parse(r.mode)?, r.content))
    }

    async fn find(&self, rev: &str, pattern: &str, limit: u32) -> Result<Vec<String>> {
        let req = FindRequest { rev: rev.to_owned(), pattern: pattern.to_owned(), limit };
        let mut c = BrowseClient::new(self.ch()).max_decoding_message_size(MAX_MESSAGE_BYTES);
        let r = c.find(req).await.map_err(st)?.into_inner();
        if r.truncated {
            eprintln!("warning: results truncated");
        }
        Ok(r.paths)
    }

    async fn blame(&self, rev: &str, path: &str) -> Result<Vec<(Id, String)>> {
        let req = BlameRequest { rev: rev.to_owned(), path: path.to_owned() };
        let mut c = HistoryClient::new(self.ch()).max_decoding_message_size(MAX_MESSAGE_BYTES);
        let r = c.blame(req).await.map_err(st)?.into_inner();
        r.lines.into_iter().map(|l| Ok((Id::from_slice(&l.changeset)?, l.text))).collect()
    }

    async fn lock(&self, path: &str) -> Result<(String, String, i64)> {
        let l = LocksClient::new(self.ch()).lock(LockRequest { path: path.to_owned() }).await.map_err(st)?.into_inner();
        Ok((l.path, l.owner, l.locked_at_ms))
    }

    async fn unlock(&self, path: &str, force: bool) -> Result<()> {
        LocksClient::new(self.ch()).unlock(UnlockRequest { path: path.to_owned(), force }).await.map_err(st)?;
        Ok(())
    }

    async fn locks(&self, prefix: &str) -> Result<Vec<(String, String, i64)>> {
        let r = LocksClient::new(self.ch()).list(ListLocksRequest { prefix: prefix.to_owned() }).await.map_err(st)?;
        Ok(r.into_inner().locks.into_iter().map(|l| (l.path, l.owner, l.locked_at_ms)).collect())
    }

    async fn whoami(&self) -> Result<ReadScope> {
        let r = AccessClient::new(self.ch()).who_am_i(Empty {}).await.map_err(st)?.into_inner();
        Ok(if r.full_read { ReadScope::All } else { ReadScope::Prefixes(r.read_prefixes) })
    }

    async fn checkout(
        &self,
        rev: &str,
        paths: &[String],
        sink: &mut (dyn FnMut(Piece) -> Result<()> + Send),
    ) -> Result<()> {
        let req = CheckoutRequest { rev: rev.to_owned(), paths: paths.to_vec() };
        let mut c = BrowseClient::new(self.ch()).max_decoding_message_size(MAX_MESSAGE_BYTES);
        let mut stream = c.checkout(req).await.map_err(st)?.into_inner();
        while let Some(p) = stream.message().await.map_err(st)? {
            validate_path(&p.path)?;
            sink(Piece {
                mode: Mode::parse(p.mode)?,
                id: Id::from_slice(&p.id)?,
                path: p.path,
                data: p.data,
                last: p.last,
            })?;
        }
        Ok(())
    }
}
