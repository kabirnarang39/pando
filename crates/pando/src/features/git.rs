//! Git interop: import a git history into Pando and export Pando history back to
//! git. Import is deterministic (same git history -> same changeset ids), so re-running
//! it is idempotent and importing new git commits fast-forwards the ref.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use pando_core::chunked::{CHUNK_THRESHOLD, chunk_reader, read_content};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Changeset, FileChange, Kind, Mode, encode_object};
use pando_core::path::components;
use pando_core::{Id, paths_overlap};

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
    /// `objects`: the blob itself, or its chunks followed by its manifest.
    Blob {
        mark: u64,
        id: Id,
        objects: Vec<(Id, Bytes)>,
    },
    Commit(GitCommit),
}

/// Objects that store one file's content: a single blob, or (for large files) its
/// chunks followed by the manifest, in upload order.
pub fn encode_blob(data: &[u8]) -> Result<(Id, Vec<(Id, Bytes)>)> {
    if (data.len() as u64) < CHUNK_THRESHOLD {
        let (id, payload) = encode_object(Kind::Blob, data);
        return Ok((id, vec![(id, payload)]));
    }
    let mut objects = Vec::new();
    let (id, manifest) = chunk_reader(data, |cid, payload| {
        objects.push((cid, payload));
        Ok(())
    })?;
    objects.push((id, manifest));
    Ok((id, objects))
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
                    encode_blob(&data).map(|(id, objects)| Item::Blob { mark, id, objects })
                }
                Ok(Some(Event::Commit(c))) => Ok(Item::Commit(c)),
                Ok(Some(Event::Reset { .. })) => continue,
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
            Item::Blob { mark, id, objects } => {
                blob_marks.insert(mark, id);
                pending_bytes += objects.iter().map(|(_, p)| p.len()).sum::<usize>();
                pending.extend(objects);
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

/// Turn one fast-export commit into a server-side changeset. `blobs` and `commits` map
/// fast-export marks to Pando ids. Shared by `import-git` and the git remote helper.
pub async fn import_commit(
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
        // git accepts a parent listed twice (the Linux kernel has four such merges); the
        // repeat adds no ancestry, so import it once.
        let p = parent_of(m)?;
        if !parents.contains(&p) {
            parents.push(p);
        }
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
                // `.git` / `.pando` paths (any case or Windows alias) are refused by name, not skipped.
                components(path).map_err(|e| anyhow::anyhow!("can't import git path `{path}`: {e}"))?;
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
    init_git(git_dir)?;
    let chain = first_parent_chain(remote, rev).await?;
    write_commits(remote, &chain, git_dir, git_ref, None).await?;
    Ok(chain.len())
}

fn init_git(git_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(git_dir)?;
    if !git_dir.join(".git").exists() && !git_dir.join("HEAD").exists() {
        git(git_dir, &["init", "-q"])?;
    }
    Ok(())
}

/// Writes `chain` (oldest first) as commits on `git_ref`, continuing from git commit
/// `continue_from` when given. Commits are deterministic: the same changesets always make the
/// same git commits.
async fn write_commits(
    remote: &dyn Remote,
    chain: &[crate::remote::RemoteChangeset],
    git_dir: &Path,
    git_ref: &str,
    continue_from: Option<&str>,
) -> Result<()> {
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
    if let Some(sha) = continue_from {
        w.reset(&refname, sha)?;
    }
    for rc in chain {
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
                let payload = payload.with_context(|| format!("blob {id} missing"))?;
                let (_, content) = read_content(objects.as_ref(), id, &payload).await?;
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
    Ok(())
}

/// Where a mirror has got to: the last changeset exported and the git commit it became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorState {
    pub changeset: Id,
    pub commit: String,
}

const MIRROR_STATE: &str = "pando-mirror";

pub fn mirror_state(git_dir: &Path) -> Result<Option<MirrorState>> {
    let path = git_dir.join(MIRROR_STATE);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
    let (cs, commit) = text.trim().split_once(' ').with_context(|| format!("{} is damaged", path.display()))?;
    Ok(Some(MirrorState { changeset: cs.parse::<Id>()?, commit: commit.to_owned() }))
}

/// Brings `git_ref` in `git_dir` up to date with `rev`, appending only changes not yet
/// exported. Returns how many commits were added (0 when already current).
pub async fn mirror_once(
    remote: &dyn Remote,
    flags: &dyn FeatureFlags,
    rev: &str,
    git_dir: &Path,
    git_ref: &str,
) -> Result<usize> {
    require(flags, Flag::GitExport)?;
    init_git(git_dir)?;
    let Some(tip) = remote.get_ref(rev).await? else { return Ok(0) };
    let state = mirror_state(git_dir)?;
    if state.as_ref().is_some_and(|s| s.changeset == tip) {
        return Ok(0);
    }
    // Changes since the last export, newest first, then reversed.
    let mut chain = Vec::new();
    let mut next = Some(tip.to_hex());
    'pages: while let Some(r) = next {
        let page = remote.log(&r, "", 10_000).await?;
        next = page.last().and_then(|c| c.changeset.first_parent().ok().flatten()).map(|p| p.to_hex());
        for rc in page {
            if state.as_ref().is_some_and(|s| s.changeset == rc.id) {
                break 'pages;
            }
            chain.push(rc);
        }
        if next.is_none() && state.is_some() {
            bail!("{rev} no longer contains the last mirrored change; refusing to rewrite the mirror");
        }
    }
    chain.reverse();
    write_commits(remote, &chain, git_dir, git_ref, state.as_ref().map(|s| s.commit.as_str())).await?;
    let commit = git(git_dir, &["rev-parse", &format!("refs/heads/{git_ref}")])?;
    // Written last: if anything above fails, the next run redoes the same work.
    let tmp = git_dir.join(format!("{MIRROR_STATE}.tmp"));
    std::fs::write(&tmp, format!("{} {commit}\n", tip.to_hex()))?;
    std::fs::rename(tmp, git_dir.join(MIRROR_STATE))?;
    Ok(chain.len())
}

/// A URL with any password or token replaced, for messages and logs.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_owned() };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://***@{}", &rest[at + 1..]),
        None => url.to_owned(),
    }
}

/// Where a mirror keeps its local copy when no directory is given: a bare repository in the
/// user's cache, one per destination.
pub fn default_mirror_dir(to: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let name: String =
        redact(to).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    base.join("pando").join("mirror").join(name)
}

/// Brings the mirror's local copy up to date with `rev` and pushes `git_ref` to `to`.
/// The push never forces: if the destination has commits the mirror didn't make, it fails
/// and says so. Pushing every time also retries a push that failed last time.
pub async fn mirror(
    remote: &dyn Remote,
    flags: &dyn FeatureFlags,
    rev: &str,
    dir: &Path,
    git_ref: &str,
    to: &str,
) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    if !dir.join("HEAD").exists() {
        git(dir, &["init", "-q", "--bare"])?;
    }
    let n = mirror_once(remote, flags, rev, dir, git_ref).await?;
    if git(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{git_ref}")]).is_err() {
        return Ok(0); // nothing has landed yet
    }
    let spec = format!("refs/heads/{git_ref}:refs/heads/{git_ref}");
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["push", "--quiet", "--porcelain", to, &spec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("running git push")?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).replace(to, &redact(to));
        let why = String::from_utf8_lossy(&out.stdout).replace(to, &redact(to)) + &why;
        if why.contains("[rejected]") || why.contains("non-fast-forward") || why.contains("fetch first") {
            bail!(
                "{} has commits on {git_ref} that did not come from this mirror; the mirror never \
                 overwrites them. Mirror into an empty repository or branch, or one this mirror made.",
                redact(to)
            );
        }
        bail!("pushing to {} failed: {}", redact(to), why.trim());
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redact_hides_credentials_only() {
        assert_eq!(
            redact("https://x-access-token:ghp_secret@github.com/acme/fleet.git"),
            "https://***@github.com/acme/fleet.git"
        );
        assert_eq!(redact("https://github.com/acme/fleet.git"), "https://github.com/acme/fleet.git");
        assert_eq!(redact("git@github.com:acme/fleet.git"), "git@github.com:acme/fleet.git");
        assert_eq!(redact("https://github.com/a/b@c"), "https://github.com/a/b@c");
    }
}
