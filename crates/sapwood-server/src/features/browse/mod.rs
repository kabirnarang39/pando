//! Browse slice: API-first file access. CI jobs and tools read files and directories at
//! any revision without cloning or checking anything out. Every operation is limited to
//! the caller's read scope, and `Checkout` is how principals with path-scoped read access
//! materialise a working copy (they may not read objects by id).

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use sapwood_core::acl::{AccessPolicy, Principal, ReadScope, effective_scope, require_read};
use sapwood_core::chunked::{is_manifest, parse_manifest};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Kind, Mode, TreeEntry, decode_object};
use sapwood_core::path::{components, components_or_root};
use sapwood_core::{Error, Id, Repo, Result, tree};
use sapwood_proto::browse_server::{self, BrowseServer};
use sapwood_proto::{
    BrowseRequest, CheckoutPiece, CheckoutRequest, DirEntry, FindRequest, FindResponse, ListDirResponse,
    ReadFileResponse,
};
use tokio::sync::mpsc::Sender;
use tonic::{Request, Response, Status};

use crate::auth::principal;
use crate::status::{RpcResult, async_stream, to_status};

pub const DEFAULT_FIND_LIMIT: u32 = 1_000;
pub const MAX_FIND_LIMIT: u32 = 100_000;
/// Bound on tree entries visited by one find, so a query cannot pin the server.
pub const MAX_FIND_VISITS: usize = 10_000_000;
/// Largest piece of file content in one `Checkout` message.
pub const PIECE_BYTES: usize = 4 << 20;

#[derive(Clone)]
pub struct Browse {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") }
}

impl Browse {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    fn scope(&self, who: &Principal) -> ReadScope {
        effective_scope(self.flags.as_ref(), self.policy.as_ref(), who)
    }

    async fn locate(&self, rev: &str, path: &str) -> Result<(Mode, Id)> {
        let id = self.repo.resolve(rev).await?;
        self.repo.lookup(id, path).await?.ok_or_else(|| Error::not_found(format!("`{path}` at {rev}")))
    }

    /// Entries the caller may see: readable ones, and directories that lead to readable
    /// paths (shown without their id, which would describe content they cannot read).
    pub async fn list_dir(&self, who: &Principal, rev: &str, path: &str) -> Result<Vec<TreeEntry>> {
        require(self.flags.as_ref(), Flag::Browse)?;
        let scope = self.scope(who);
        let dir = components_or_root(path)?.join("/");
        if !scope.traversable(&dir) {
            return Err(Error::PermissionDenied(format!("{} may not read `{path}`", who.0)));
        }
        let entries = match self.locate(rev, path).await? {
            (Mode::Dir, id) => tree::read_tree(self.repo.objects.as_ref(), &id).await?.entries,
            _ => return Err(Error::invalid(format!("`{path}` is not a directory"))),
        };
        if scope.is_full() {
            return Ok(entries);
        }
        let mut visible = Vec::new();
        for mut e in entries {
            let child = join(&dir, &e.name);
            if scope.allows(&child) {
                visible.push(e);
            } else if Mode::parse(e.mode)? == Mode::Dir && scope.traversable(&child) {
                e.id.clear();
                visible.push(e);
            }
        }
        Ok(visible)
    }

    pub async fn read_file(&self, who: &Principal, rev: &str, path: &str) -> Result<(Mode, Id, Vec<u8>)> {
        require(self.flags.as_ref(), Flag::Browse)?;
        require_read(&self.scope(who), who, path)?;
        match self.locate(rev, path).await? {
            (Mode::Dir, _) => Err(Error::invalid(format!("`{path}` is a directory"))),
            (mode, id) => Ok((mode, id, tree::read_object(self.repo.objects.as_ref(), &id, Kind::Blob).await?)),
        }
    }

    /// Breadth-first walk of the readable part of the tree at `rev`, one batched fetch per
    /// level. Calls `visit(path, mode, id)` for each file; stops when it returns false.
    async fn walk_files(
        &self,
        scope: &ReadScope,
        want: &ReadScope,
        rev: &str,
        mut visit: impl FnMut(String, Mode, Id) -> Result<bool>,
    ) -> Result<bool> {
        let root = self.repo.root_tree(self.repo.resolve(rev).await?).await?;
        let objects = self.repo.objects.as_ref();
        let (mut level, mut visits) = (vec![(String::new(), root)], 0usize);
        while !level.is_empty() {
            let mut next = Vec::new();
            let ids: Vec<Id> = level.iter().map(|(_, id)| *id).collect();
            let payloads = objects.get_many(&ids).await?;
            for ((prefix, id), payload) in level.into_iter().zip(payloads) {
                let payload = payload.ok_or_else(|| Error::not_found(format!("tree {id}")))?;
                let (_, content) = decode_object(&id, &payload)?;
                for e in tree::parse_tree(&id, &content)?.entries {
                    visits += 1;
                    if visits >= MAX_FIND_VISITS {
                        return Ok(false);
                    }
                    let path = join(&prefix, &e.name);
                    let mode = Mode::parse(e.mode)?;
                    if mode == Mode::Dir {
                        if scope.traversable(&path) && want.traversable(&path) {
                            next.push((path, Id::from_slice(&e.id)?));
                        }
                    } else if scope.allows(&path) && want.allows(&path) && !visit(path, mode, Id::from_slice(&e.id)?)? {
                        return Ok(false);
                    }
                }
            }
            level = next;
        }
        Ok(true)
    }

    /// Readable paths containing `pattern` at `rev`. Returns (paths, truncated).
    pub async fn find(&self, who: &Principal, rev: &str, pattern: &str, limit: u32) -> Result<(Vec<String>, bool)> {
        require(self.flags.as_ref(), Flag::Search)?;
        if pattern.is_empty() {
            return Err(Error::invalid("empty pattern"));
        }
        let limit = if limit == 0 { DEFAULT_FIND_LIMIT } else { limit.min(MAX_FIND_LIMIT) } as usize;
        let mut found = Vec::new();
        let complete = self
            .walk_files(&self.scope(who), &ReadScope::All, rev, |path, _, _| {
                if path.contains(pattern) {
                    found.push(path);
                }
                Ok(found.len() < limit)
            })
            .await?;
        found.sort();
        Ok((found, !complete))
    }

    /// Stream every readable file under `paths` (all readable files when empty).
    pub async fn checkout(
        &self,
        who: &Principal,
        rev: &str,
        paths: &[String],
        tx: &Sender<Result<CheckoutPiece, Status>>,
    ) -> Result<()> {
        require(self.flags.as_ref(), Flag::Browse)?;
        let want = if paths.is_empty() {
            ReadScope::All
        } else {
            for p in paths {
                components(p)?;
            }
            ReadScope::Prefixes(paths.to_vec())
        };
        let mut files = Vec::new();
        self.walk_files(&self.scope(who), &want, rev, |path, mode, id| {
            files.push((path, mode, id));
            Ok(true)
        })
        .await?;
        let objects = self.repo.objects.as_ref();
        for (path, mode, id) in files {
            let payload = objects.get(&id).await?.ok_or_else(|| Error::not_found(format!("object {id}")))?;
            let piece = |data: Vec<u8>, last: bool| CheckoutPiece {
                path: path.clone(),
                mode: mode as i32,
                id: id.0.to_vec(),
                data,
                last,
            };
            let mut pieces: Vec<CheckoutPiece> = Vec::new();
            if is_manifest(&payload) {
                let m = parse_manifest(&payload)?;
                let n = m.chunks.len();
                for (i, c) in m.chunks.iter().enumerate() {
                    let cid = Id::from_slice(&c.id)?;
                    let chunk = objects.get(&cid).await?.ok_or_else(|| Error::not_found(format!("chunk {cid}")))?;
                    let (_, data) = decode_object(&cid, &chunk)?;
                    if tx.send(Ok(piece(data, i + 1 == n))).await.is_err() {
                        return Ok(()); // client went away
                    }
                }
                if n == 0 {
                    pieces.push(piece(Vec::new(), true));
                }
            } else {
                let (_, data) = decode_object(&id, &payload)?;
                let parts: Vec<&[u8]> = if data.is_empty() { vec![&[]] } else { data.chunks(PIECE_BYTES).collect() };
                let n = parts.len();
                pieces.extend(parts.into_iter().enumerate().map(|(i, d)| piece(d.to_vec(), i + 1 == n)));
            }
            for p in pieces {
                if tx.send(Ok(p)).await.is_err() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

pub fn service(s: Browse) -> BrowseServer<Browse> {
    BrowseServer::new(s).max_encoding_message_size(sapwood_proto::MAX_MESSAGE_BYTES)
}

#[tonic::async_trait]
impl browse_server::Browse for Browse {
    type CheckoutStream = Pin<Box<dyn Stream<Item = std::result::Result<CheckoutPiece, Status>> + Send>>;

    async fn list_dir(&self, req: Request<BrowseRequest>) -> RpcResult<ListDirResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let entries = Browse::list_dir(self, &who, &r.rev, &r.path).await.map_err(to_status)?;
        Ok(Response::new(ListDirResponse {
            entries: entries.into_iter().map(|e| DirEntry { name: e.name, mode: e.mode, id: e.id }).collect(),
        }))
    }

    async fn read_file(&self, req: Request<BrowseRequest>) -> RpcResult<ReadFileResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let (mode, id, content) = Browse::read_file(self, &who, &r.rev, &r.path).await.map_err(to_status)?;
        Ok(Response::new(ReadFileResponse { content, mode: mode as i32, id: id.0.to_vec() }))
    }

    async fn find(&self, req: Request<FindRequest>) -> RpcResult<FindResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let (paths, truncated) = Browse::find(self, &who, &r.rev, &r.pattern, r.limit).await.map_err(to_status)?;
        Ok(Response::new(FindResponse { paths, truncated }))
    }

    async fn checkout(&self, req: Request<CheckoutRequest>) -> RpcResult<Self::CheckoutStream> {
        let who = principal(&req);
        let r = req.into_inner();
        let this = self.clone();
        let stream = async_stream(move |tx| async move {
            if let Err(e) = this.checkout(&who, &r.rev, &r.paths, &tx).await {
                let _ = tx.send(Err(to_status(e))).await;
            }
        });
        Ok(Response::new(Box::pin(stream)))
    }
}
