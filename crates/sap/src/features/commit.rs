use anyhow::{Result, bail};
use bytes::Bytes;
use sapwood_core::Id;
use sapwood_core::object::{Changeset, FileChange};

use crate::remote::Remote;
use crate::workspace::{ChangeKind, Workspace};

/// Upload batch ceiling (compressed bytes) per Put stream.
const UPLOAD_BATCH_BYTES: usize = 32 << 20;

#[derive(Debug)]
pub struct Committed {
    pub id: Id,
    pub files: usize,
}

/// Upload changed blobs (only those the server lacks) and create a changeset on the
/// server with the workspace base as parent. `pathspecs` limits the commit to matching
/// paths (git's staging, without a staging area).
pub async fn run(remote: &dyn Remote, ws: &mut Workspace, message: &str, author: &str, pathspecs: &[String]) -> Result<Option<Committed>> {
    if message.trim().is_empty() {
        bail!("empty commit message (use -m)");
    }
    let changes = ws.status(pathspecs)?;
    if changes.is_empty() {
        return Ok(None);
    }
    let upserts: Vec<String> = changes.iter().filter(|c| c.kind != ChangeKind::Deleted).map(|c| c.path.clone()).collect();
    // Re-read and encode now: the encoded id is authoritative even if a file changed
    // since the status scan.
    let encoded = ws.encode_files(&upserts)?;
    upload(remote, &encoded).await?;

    let mut file_changes = Vec::with_capacity(changes.len());
    let mut enc = encoded.iter();
    for c in &changes {
        if c.kind == ChangeKind::Deleted {
            file_changes.push(FileChange::delete(c.path.clone()));
        } else {
            let (id, _) = enc.next().expect("one encoding per upsert");
            let (mode, _) = c.new.expect("upsert has mode");
            file_changes.push(FileChange::upsert(c.path.clone(), mode, *id));
        }
    }
    let cs = Changeset {
        parents: ws.base.map(|b| vec![b.0.to_vec()]).unwrap_or_default(),
        changes: file_changes.clone(),
        author: author.to_owned(),
        message: message.to_owned(),
        timestamp_ms: crate::now_ms(),
    };
    let created = remote.create_changeset(cs).await?;
    for c in &file_changes {
        match c.blob_id()? {
            Some(id) => ws.mark_clean(&c.path, sapwood_core::object::Mode::parse(c.mode)?, id)?,
            None => {
                ws.index.remove(&c.path);
            }
        }
    }
    ws.base = Some(created.id);
    ws.base_landed = false;
    ws.save()?;
    Ok(Some(Committed { id: created.id, files: file_changes.len() }))
}

/// Upload only what the server is missing, in bounded batches.
pub async fn upload(remote: &dyn Remote, objects: &[(Id, Bytes)]) -> Result<usize> {
    if objects.is_empty() {
        return Ok(0);
    }
    let store = remote.objects();
    let ids: Vec<Id> = objects.iter().map(|(i, _)| *i).collect();
    let missing: std::collections::HashSet<Id> = store.missing(&ids).await?.into_iter().collect();
    let mut batch = Vec::new();
    let (mut size, mut sent) = (0, 0);
    for (id, payload) in objects.iter().filter(|(i, _)| missing.contains(i)) {
        size += payload.len();
        batch.push((*id, payload.clone()));
        if size >= UPLOAD_BATCH_BYTES {
            sent += batch.len();
            store.put_many(std::mem::take(&mut batch)).await?;
            size = 0;
        }
    }
    sent += batch.len();
    store.put_many(batch).await?;
    Ok(sent)
}
