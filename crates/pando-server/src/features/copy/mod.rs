//! Copy a repository between stores: backups, restores, and moving to another backend
//! (SQLite to Postgres, disk to S3). Safe while the source is serving.
//!
//! Order matters. Branch pointers are read first, then changesets, objects and derived
//! records are copied, and the pointers are written last. Everything a pointer refers to was
//! written before the pointer moved (ADR-018), so it is already in the source when the copy
//! starts listing, and the destination never points at something it does not hold. Objects
//! are copied as stored, so compacted (delta) objects stay compact, and anything the
//! destination already holds is skipped, which makes a repeated copy incremental.

use std::collections::HashSet;

use futures::{StreamExt, TryStreamExt, stream};
use pando_core::object::{Changeset, decode_object};
use pando_core::{Derivation, Generation, RootTree};
use pando_core::{Error, Id, Repo, Result, SYSTEM_REF_PREFIX, delta};

use crate::features::compact::{DONE_KIND, PIN_KIND};

const PAGE: usize = 1000;
const CONCURRENCY: usize = 32;

/// Derived records worth copying: recomputing them would mean replaying all history.
const DERIVED: [&str; 5] = [RootTree::KIND, Generation::KIND, "landed-as.v1", DONE_KIND, PIN_KIND];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyStats {
    pub refs: usize,
    pub changesets: usize,
    pub objects_copied: usize,
    pub objects_present: usize,
    pub derived: usize,
}

/// Copy everything in `from` to `to`. Both must be raw stores (no caches), as returned in
/// `Storage::raw`.
pub async fn copy(from: &Repo, to: &Repo) -> Result<CopyStats> {
    let mut stats = CopyStats::default();
    let refs: Vec<_> =
        from.refs.list().await?.into_iter().filter(|(name, _)| !name.starts_with(SYSTEM_REF_PREFIX)).collect();

    // Changesets, verified against their ids.
    let mut after = None;
    loop {
        let ids = from.log.list(after, PAGE).await?;
        let Some(last) = ids.last().copied() else { break };
        // Both sides list in id order, so the destination's page from the same point says
        // which of these it already holds.
        let held: HashSet<Id> = to.log.list(after, PAGE).await?.into_iter().collect();
        let new: Vec<Id> = ids.iter().copied().filter(|id| !held.contains(id)).collect();
        stats.changesets += new.len();
        stream::iter(new)
            .map(|id| async move {
                let bytes = from.log.get(&id).await?.ok_or_else(|| Error::not_found(format!("changeset {id}")))?;
                Changeset::decode_verified(&id, &bytes)?;
                to.log.append(id, bytes).await
            })
            .buffer_unordered(CONCURRENCY)
            .try_collect::<()>()
            .await?;
        after = Some(last);
        if ids.len() == PAGE && last.0[0] != ids[0].0[0] {
            tracing::info!(changesets = stats.changesets, at = %last.short(), "copying changesets");
        }
    }

    // Objects, as stored. Whole objects are verified; deltas are verified when read.
    let mut after = None;
    loop {
        let ids = from.objects.list(after, PAGE).await?;
        let Some(last) = ids.last().copied() else { break };
        let missing = to.objects.missing(&ids).await?;
        stats.objects_present += ids.len() - missing.len();
        let payloads = from.objects.get_many(&missing).await?;
        let mut batch = Vec::with_capacity(missing.len());
        for (id, payload) in missing.into_iter().zip(payloads) {
            let payload = payload.ok_or_else(|| Error::not_found(format!("object {id}")))?;
            if !delta::is_delta(&payload) {
                decode_object(&id, &payload)?;
            }
            batch.push((id, payload));
        }
        stats.objects_copied += batch.len();
        to.objects.put_many(batch).await?;
        after = Some(last);
        if ids.len() == PAGE && last.0[0] != ids[0].0[0] {
            tracing::info!(copied = stats.objects_copied, present = stats.objects_present, at = %last.short(), "copying objects");
        }
    }

    for kind in DERIVED {
        let mut after = None;
        loop {
            let page = from.derived.list(kind, after, PAGE).await?;
            let Some((last, _)) = page.last().cloned() else { break };
            let held: HashSet<Id> = to.derived.list(kind, after, PAGE).await?.into_iter().map(|(id, _)| id).collect();
            after = Some(last);
            let page: Vec<_> = page.into_iter().filter(|(id, _)| !held.contains(id)).collect();
            stats.derived += page.len();
            stream::iter(page)
                .map(|(id, value)| async move { to.derived.put(kind, id, value).await })
                .buffer_unordered(CONCURRENCY)
                .try_collect::<()>()
                .await?;
        }
    }

    // Pointers last, overwriting whatever the destination had.
    for (name, id) in refs {
        loop {
            let current = to.refs.get(&name).await?;
            if current == Some(id) || to.refs.compare_and_swap(&name, current, id).await? {
                break;
            }
        }
        stats.refs += 1;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pando_adapters::memory::MemoryStore;
    use pando_adapters::sqlite::SqliteStore;
    use pando_core::object::{FileChange, Mode};
    use pando_core::tree;

    use super::*;

    fn repo<
        S: pando_core::ports::ObjectStore
            + pando_core::ports::ChangesetLog
            + pando_core::ports::RefStore
            + pando_core::ports::DerivedStore
            + 'static,
    >(
        s: Arc<S>,
    ) -> Repo {
        Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copies_a_repository_and_repeats_incrementally() {
        let from = repo(Arc::new(MemoryStore::new()));
        let mut head = None;
        for i in 0..30u32 {
            let blob = tree::put_blob(from.objects.as_ref(), format!("v{i}").as_bytes()).await.unwrap();
            let cs = from
                .create_changeset(Changeset {
                    parents: head.map(|h: pando_core::Id| vec![h.0.to_vec()]).unwrap_or_default(),
                    changes: vec![FileChange::upsert(format!("dir{}/f{i}", i % 3), Mode::File, blob)],
                    author: "t".into(),
                    message: format!("{i}"),
                    timestamp_ms: 0,
                })
                .await
                .unwrap();
            head = Some(cs);
        }
        let head = head.unwrap();
        from.refs.compare_and_swap("main", None, head).await.unwrap();
        from.refs.compare_and_swap("_pando/compaction-lease", None, head).await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let to = repo(Arc::new(SqliteStore::open(dir.path().join("copy.db")).unwrap()));
        let first = copy(&from, &to).await.unwrap();
        assert_eq!((first.refs, first.changesets), (1, 30));
        assert!(first.objects_copied > 30, "blobs and trees: {first:?}");
        assert!(first.derived >= 60, "root trees and graph records: {first:?}");

        // The copy serves the same history and content, and system refs stay behind.
        assert_eq!(to.refs.get("main").await.unwrap(), Some(head));
        assert_eq!(to.refs.get("_pando/compaction-lease").await.unwrap(), None);
        let (a, b) = (from.root_tree(head).await.unwrap(), to.root_tree(head).await.unwrap());
        assert_eq!(a, b);
        let files = tree::diff_trees(to.objects.as_ref(), None, Some(b)).await.unwrap();
        assert_eq!(files.len(), 30);

        // Copying again moves nothing new.
        let again = copy(&from, &to).await.unwrap();
        assert_eq!((again.objects_copied, again.objects_present), (0, first.objects_copied));
        assert_eq!((again.changesets, again.derived), (0, 0));
    }
}
