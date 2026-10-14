//! Browse slice: API-first file access. CI jobs and tools read files and directories at
//! any revision without cloning or checking anything out.

use std::sync::Arc;

use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Kind, Mode, TreeEntry, decode_object};
use sapwood_core::{Error, Id, Repo, Result, tree};
use sapwood_proto::browse_server::{self, BrowseServer};
use sapwood_proto::{BrowseRequest, DirEntry, FindRequest, FindResponse, ListDirResponse, ReadFileResponse};
use tonic::{Request, Response};

use crate::status::{RpcResult, to_status};

pub const DEFAULT_FIND_LIMIT: u32 = 1_000;
pub const MAX_FIND_LIMIT: u32 = 100_000;
/// Bound on tree entries visited by one find, so a query cannot pin the server.
pub const MAX_FIND_VISITS: usize = 10_000_000;

#[derive(Clone)]
pub struct Browse {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
}

impl Browse {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>) -> Self {
        Self { repo, flags }
    }

    async fn locate(&self, rev: &str, path: &str) -> Result<(Mode, Id)> {
        let id = self.repo.resolve(rev).await?;
        self.repo.lookup(id, path).await?.ok_or_else(|| Error::not_found(format!("`{path}` at {rev}")))
    }

    pub async fn list_dir(&self, rev: &str, path: &str) -> Result<Vec<TreeEntry>> {
        require(self.flags.as_ref(), Flag::Browse)?;
        match self.locate(rev, path).await? {
            (Mode::Dir, id) => Ok(tree::read_tree(self.repo.objects.as_ref(), &id).await?.entries),
            _ => Err(Error::invalid(format!("`{path}` is not a directory"))),
        }
    }

    pub async fn read_file(&self, rev: &str, path: &str) -> Result<(Mode, Id, Vec<u8>)> {
        require(self.flags.as_ref(), Flag::Browse)?;
        match self.locate(rev, path).await? {
            (Mode::Dir, _) => Err(Error::invalid(format!("`{path}` is a directory"))),
            (mode, id) => Ok((mode, id, tree::read_object(self.repo.objects.as_ref(), &id, Kind::Blob).await?)),
        }
    }

    /// Paths containing `pattern` at `rev`, breadth-first, one batched fetch per level.
    /// Returns (paths, truncated).
    pub async fn find(&self, rev: &str, pattern: &str, limit: u32) -> Result<(Vec<String>, bool)> {
        require(self.flags.as_ref(), Flag::Search)?;
        if pattern.is_empty() {
            return Err(Error::invalid("empty pattern"));
        }
        let limit = if limit == 0 { DEFAULT_FIND_LIMIT } else { limit.min(MAX_FIND_LIMIT) } as usize;
        let root = self.repo.root_tree(self.repo.resolve(rev).await?).await?;
        let objects = self.repo.objects.as_ref();
        let (mut level, mut found, mut visits) = (vec![(String::new(), root)], Vec::new(), 0usize);
        while !level.is_empty() {
            let mut next = Vec::new();
            let ids: Vec<Id> = level.iter().map(|(_, id)| *id).collect();
            let payloads = objects.get_many(&ids).await?;
            for ((prefix, id), payload) in level.into_iter().zip(payloads) {
                let payload = payload.ok_or_else(|| Error::not_found(format!("tree {id}")))?;
                let (_, content) = decode_object(&id, &payload)?;
                for e in tree::parse_tree(&id, &content)?.entries {
                    visits += 1;
                    let path = if prefix.is_empty() { e.name } else { format!("{prefix}/{}", e.name) };
                    if Mode::parse(e.mode)? == Mode::Dir {
                        next.push((path, Id::from_slice(&e.id)?));
                    } else if path.contains(pattern) {
                        found.push(path);
                        if found.len() >= limit {
                            return Ok((found, true));
                        }
                    }
                    if visits >= MAX_FIND_VISITS {
                        return Ok((found, true));
                    }
                }
            }
            level = next;
        }
        found.sort();
        Ok((found, false))
    }
}

pub fn service(s: Browse) -> BrowseServer<Browse> {
    BrowseServer::new(s).max_encoding_message_size(sapwood_proto::MAX_MESSAGE_BYTES)
}

#[tonic::async_trait]
impl browse_server::Browse for Browse {
    async fn list_dir(&self, req: Request<BrowseRequest>) -> RpcResult<ListDirResponse> {
        let r = req.into_inner();
        let entries = Browse::list_dir(self, &r.rev, &r.path).await.map_err(to_status)?;
        Ok(Response::new(ListDirResponse {
            entries: entries.into_iter().map(|e| DirEntry { name: e.name, mode: e.mode, id: e.id }).collect(),
        }))
    }

    async fn read_file(&self, req: Request<BrowseRequest>) -> RpcResult<ReadFileResponse> {
        let r = req.into_inner();
        let (mode, id, content) = Browse::read_file(self, &r.rev, &r.path).await.map_err(to_status)?;
        Ok(Response::new(ReadFileResponse { content, mode: mode as i32, id: id.0.to_vec() }))
    }

    async fn find(&self, req: Request<FindRequest>) -> RpcResult<FindResponse> {
        let r = req.into_inner();
        let (paths, truncated) = Browse::find(self, &r.rev, &r.pattern, r.limit).await.map_err(to_status)?;
        Ok(Response::new(FindResponse { paths, truncated }))
    }
}
