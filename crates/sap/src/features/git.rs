//! Git interop: import a git history into Sapwood and export Sapwood history back to
//! git. Import is deterministic (same git history -> same changeset ids), so re-running
//! it is idempotent and importing new git commits fast-forwards the ref.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Changeset, FileChange, Kind, Mode, decode_object, encode_object};
use sapwood_core::path::components;
use sapwood_core::{Id, paths_overlap};

use super::commit::upload;
use crate::gitfe::{DataRef, Event, GitCommit, Op, Parser, Writer};
use crate::remote::Remote;
use crate::workspace::list_files;

const UPLOAD_FLUSH_BYTES: usize = 32 << 20;

#[derive(Debug, Default)]
pub struct ImportOutcome {
    pub commits: usize,
    pub blobs: usize,
    pub uploaded: usize,
    pub skipped_submodules: usize,
    pub tip: Option<Id>,
}

/// Parsed stream events with blobs already compressed and hashed (on the parser thread).
enum Item {
    Blob { mark: u64, id: Id, payload: Bytes },
    Commit(GitCommit),
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

fn git_mode(mode: u32) -> Result<Option<Mode>> {
    Ok(match mode {
        0o100644 | 0o100664 => Some(Mode::File),
        0o100755 => Some(Mode::Exec),
        0o120000 => Some(Mode::Symlink),
        0o160000 => None, // submodule (gitlink): not imported
        m => bail!("unsupported git mode {m:o}"),
    })
}

pub async fn import(
    remote: &dyn Remote,
    flags: &dyn FeatureFlags,
    git_dir: &Path,
    git_ref: &str,
    target_ref: &str,
    force: bool,
) -> Result<ImportOutcome> {
    require(flags, Flag::GitImport)?;
    let tip_sha = git(git_dir, &["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")])?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args([
            "fast-export",
            "--reencode=yes",
            "--signed-tags=strip",
            "--tag-of-filtered-object=drop",
            "--show-original-ids",
            git_ref,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning git fast-export")?;
    let stdout = child.stdout.take().expect("piped");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Item>>(256);
    let parser = std::thread::spawn(move || {
        let mut p = Parser::new(BufReader::with_capacity(1 << 20, stdout));
        loop {
            let item = match p.next_event() {
                Ok(None) => return,
                Ok(Some(Event::Blob { mark, data })) => {
                    let (id, payload) = encode_object(Kind::Blob, &data);
                    Ok(Item::Blob { mark, id, payload })
                }
                Ok(Some(Event::Commit(c))) => Ok(Item::Commit(c)),
                Err(e) => Err(e),
            };
            let stop = item.is_err();
            if tx.blocking_send(item).is_err() || stop {
                return;
            }
        }
    });

    let mut out = ImportOutcome::default();
    let mut blob_marks: HashMap<u64, Id> = HashMap::new();
    let mut commit_marks: HashMap<u64, Id> = HashMap::new();
    let mut by_oid: HashMap<String, Id> = HashMap::new();
    let mut pending: Vec<(Id, Bytes)> = Vec::new();
    let mut pending_bytes = 0;

    while let Some(item) = rx.recv().await {
        match item? {
            Item::Blob { mark, id, payload } => {
                blob_marks.insert(mark, id);
                pending_bytes += payload.len();
                pending.push((id, payload));
                out.blobs += 1;
                if pending_bytes >= UPLOAD_FLUSH_BYTES {
                    out.uploaded += upload(remote, &std::mem::take(&mut pending)).await?;
                    pending_bytes = 0;
                }
            }
            Item::Commit(c) => {
                out.uploaded += upload(remote, &std::mem::take(&mut pending)).await?;
                pending_bytes = 0;
                let id = import_commit(remote, &c, &blob_marks, &commit_marks, &mut out).await?;
                if let Some(m) = c.mark {
                    commit_marks.insert(m, id);
                }
                if let Some(oid) = c.original_oid {
                    by_oid.insert(oid, id);
                }
                out.commits += 1;
                if out.commits % 1000 == 0 {
                    eprintln!("  imported {} commits", out.commits);
                }
            }
        }
    }
    parser.join().map_err(|_| anyhow::anyhow!("fast-export parser panicked"))?;
    let status = child.wait()?;
    if !status.success() {
        bail!("git fast-export failed: {status}");
    }
    let tip = *by_oid.get(&tip_sha).context("tip commit missing from fast-export stream")?;
    out.tip = Some(tip);
    move_ref(remote, target_ref, tip, force).await?;
    Ok(out)
}

async fn move_ref(remote: &dyn Remote, name: &str, tip: Id, force: bool) -> Result<()> {
    let current = remote.get_ref(name).await?;
    match current {
        Some(c) if c == tip => return Ok(()),
        Some(c) if !force && remote.merge_base(&c.to_hex(), &tip.to_hex()).await? != Some(c) => {
            bail!("ref `{name}` has diverged from the imported history; pass --force to overwrite");
        }
        _ => {}
    }
    if !remote.compare_and_swap(name, current, tip).await? {
        bail!("ref `{name}` moved concurrently; re-run the import");
    }
    Ok(())
}

async fn import_commit(
    remote: &dyn Remote,
    c: &GitCommit,
    blobs: &HashMap<u64, Id>,
    commits: &HashMap<u64, Id>,
    out: &mut ImportOutcome,
) -> Result<Id> {
    let parent_of = |m: &u64| commits.get(m).copied().with_context(|| format!("unknown parent mark :{m}"));
    let mut parents = Vec::new();
    if let Some(f) = &c.from {
        parents.push(parent_of(f)?);
    }
    for m in &c.merges {
        parents.push(parent_of(m)?);
    }
    let mut files: BTreeMap<String, Option<(Mode, Id)>> = BTreeMap::new();
    let mut delete_all = false;
    for op in &c.ops {
        match op {
            Op::Modify { mode, data, path } => {
                let Some(mode) = git_mode(*mode)? else {
                    out.skipped_submodules += 1;
                    continue;
                };
                let DataRef::Mark(m) = data else { bail!("{path}: blob by sha is unsupported") };
                components(path).with_context(|| format!("git path `{path}` is not representable"))?;
                files.insert(
                    path.clone(),
                    Some((mode, *blobs.get(m).with_context(|| format!("unknown blob mark :{m}"))?)),
                );
            }
            Op::Delete(p) => {
                files.insert(p.clone(), None);
            }
            Op::DeleteAll => {
                files.clear();
                delete_all = true;
            }
        }
    }
    if delete_all && let Some(p) = parents.first() {
        let root = remote.changeset(&p.to_hex()).await?.root_tree;
        for (path, _) in list_files(remote.objects().as_ref(), root, &[]).await? {
            files.entry(path).or_insert(None);
        }
    }
    // git may delete `a` and add `a/b` in one commit; the upsert alone already replaces.
    let upserts: Vec<String> = files.iter().filter(|(_, v)| v.is_some()).map(|(k, _)| k.clone()).collect();
    files.retain(|p, v| v.is_some() || !upserts.iter().any(|u| paths_overlap(u, p)));

    let cs = Changeset {
        parents: parents.iter().map(|p| p.0.to_vec()).collect(),
        changes: files
            .into_iter()
            .map(|(p, v)| match v {
                Some((m, id)) => FileChange::upsert(p, m, id),
                None => FileChange::delete(p),
            })
            .collect(),
        author: c.author.clone(),
        message: c.message.clone(),
        timestamp_ms: c.timestamp_s * 1000,
    };
    Ok(remote.create_changeset(cs).await?.id)
}

/// First-parent chain ending at `rev`, oldest first.
async fn first_parent_chain(remote: &dyn Remote, rev: &str) -> Result<Vec<crate::remote::RemoteChangeset>> {
    let mut chain = Vec::new();
    let mut next = Some(rev.to_owned());
    while let Some(r) = next {
        let page = remote.log(&r, "", 10_000).await?;
        next = page.last().and_then(|c| c.changeset.first_parent().ok().flatten()).map(|p| p.to_hex());
        chain.extend(page);
    }
    chain.reverse();
    Ok(chain)
}

/// Export the first-parent history of `rev` into git repository `git_dir` (created if
/// missing) as branch `git_ref`. Merge changesets become ordinary commits with the
/// same content. Returns the number of commits written.
pub async fn export(
    remote: &dyn Remote,
    flags: &dyn FeatureFlags,
    rev: &str,
    git_dir: &Path,
    git_ref: &str,
) -> Result<usize> {
    require(flags, Flag::GitExport)?;
    std::fs::create_dir_all(git_dir)?;
    if !git_dir.join(".git").exists() && !git_dir.join("HEAD").exists() {
        git(git_dir, &["init", "-q"])?;
    }
    let chain = first_parent_chain(remote, rev).await?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        // On case-insensitive filesystems git defaults to core.ignorecase=true, which makes
        // fast-import fold case-only renames onto the old name. History must be exact.
        .args(["-c", "core.ignorecase=false", "fast-import", "--quiet", "--force"])
        .stdin(Stdio::piped())
        .spawn()
        .context("spawning git fast-import")?;
    let mut w = Writer { w: BufWriter::with_capacity(1 << 20, child.stdin.take().expect("piped")) };
    let objects = remote.objects();
    let mut blob_marks: HashMap<Id, u64> = HashMap::new();
    let mut next_mark = 1u64;
    let mut prev: Option<u64> = None;
    let refname = format!("refs/heads/{git_ref}");
    for rc in &chain {
        let cs = &rc.changeset;
        let new_blobs: Vec<Id> = cs
            .changes
            .iter()
            .filter_map(|c| c.blob_id().ok().flatten())
            .filter(|id| !blob_marks.contains_key(id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for chunk in new_blobs.chunks(512) {
            for (id, payload) in chunk.iter().zip(objects.get_many(chunk).await?) {
                let (_, content) = decode_object(id, &payload.with_context(|| format!("blob {id} missing"))?)?;
                w.blob(next_mark, &content)?;
                blob_marks.insert(*id, next_mark);
                next_mark += 1;
            }
        }
        let ops: Vec<(String, Option<(u32, u64)>)> = cs
            .changes
            .iter()
            .map(|c| {
                let op = match c.blob_id()? {
                    None => None,
                    Some(id) => {
                        let mode = match Mode::parse(c.mode)? {
                            Mode::Exec => 0o100755,
                            Mode::Symlink => 0o120000,
                            _ => 0o100644,
                        };
                        Some((mode, blob_marks[&id]))
                    }
                };
                Ok((c.path.clone(), op))
            })
            .collect::<Result<_>>()?;
        let mark = next_mark;
        next_mark += 1;
        w.commit(&refname, mark, &cs.author, cs.timestamp_ms.div_euclid(1000), &cs.message, prev, &ops)?;
        prev = Some(mark);
    }
    w.w.flush()?;
    drop(w);
    let status = child.wait()?;
    if !status.success() {
        bail!("git fast-import failed: {status}");
    }
    Ok(chain.len())
}
