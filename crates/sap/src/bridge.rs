//! Two-way git bridge: the `git-remote-sapwood` remote helper.
//!
//! With the helper on `PATH`, plain git works against a Sapwood server:
//!
//! ```text
//! git clone sapwood::http://server:7420 repo
//! git push / git pull / git fetch
//! ```
//!
//! Fetch streams only changesets git does not have yet (git fast-import, inline file
//! contents). Push parses git's fast-export stream into server-side changesets and lands
//! them: if the branch moved, the server rebases instead of rejecting the push
//! (`SAPWOOD_PUSH=strict` restores git's reject-if-behind behaviour).
//!
//! State lives in `$GIT_DIR/sapwood/<remote>/`: `git.marks` (owned by git's fast-import /
//! fast-export) and `sapwood.marks` (mark -> Sapwood id for every commit and blob).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sapwood_core::Id;
use sapwood_core::chunked::read_content;
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{FileChange, Mode};

use crate::features::commit::upload;
use crate::features::git::{ImportOutcome, encode_blob, import_commit};
use crate::gitfe::{Event, InlineOp, Parser, Writer};
use crate::remote::Remote;
use crate::workspace::list_files;

/// mark -> ('c' changeset | 'b' blob, id), persisted next to git's own marks file.
#[derive(Default)]
struct Marks {
    entries: BTreeMap<u64, (char, Id)>,
}

fn git_marks(path: &Path) -> Result<BTreeMap<u64, String>> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    Ok(text
        .lines()
        .filter_map(|l| {
            let (m, sha) = l.split_once(' ')?;
            Some((m.strip_prefix(':')?.parse().ok()?, sha.to_owned()))
        })
        .collect())
}

impl Marks {
    /// Load our marks, dropping entries git does not know (e.g. a fast-import that failed
    /// after we recorded its marks), so we never emit `from` a mark git cannot resolve.
    fn load(path: &Path, git: &BTreeMap<u64, String>) -> Result<Marks> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let mut it = line.split(' ');
            let (Some(m), Some(k), Some(id)) = (it.next(), it.next(), it.next()) else { continue };
            let Some(mark) = m.strip_prefix(':').and_then(|m| m.parse::<u64>().ok()) else { continue };
            let kind = k.chars().next().unwrap_or('?');
            if git.contains_key(&mark) || kind == 'b' {
                entries.insert(mark, (kind, id.parse()?));
            }
        }
        Ok(Marks { entries })
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut out = String::new();
        for (m, (k, id)) in &self.entries {
            out.push_str(&format!(":{m} {k} {id}\n"));
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    fn by_kind(&self, kind: char) -> HashMap<u64, Id> {
        self.entries.iter().filter(|(_, (k, _))| *k == kind).map(|(m, (_, id))| (*m, *id)).collect()
    }

    fn commit_mark(&self, id: &Id) -> Option<u64> {
        self.entries.iter().find(|(_, (k, i))| *k == 'c' && i == id).map(|(m, _)| *m)
    }
}

pub struct Bridge<'a> {
    remote: &'a dyn Remote,
    flags: &'a dyn FeatureFlags,
    alias: String,
    git_marks: PathBuf,
    our_marks: PathBuf,
    strict_push: bool,
}

fn git_mode(m: Mode) -> u32 {
    match m {
        Mode::Exec => 0o100755,
        Mode::Symlink => 0o120000,
        _ => 0o100644,
    }
}

impl<'a> Bridge<'a> {
    /// `alias` is the git remote name (or `_` for a bare URL); `git_dir` is `$GIT_DIR`.
    pub fn new(remote: &'a dyn Remote, flags: &'a dyn FeatureFlags, alias: &str, git_dir: &Path) -> Result<Self> {
        let safe: String = alias.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let dir = git_dir.join("sapwood").join(&safe);
        std::fs::create_dir_all(&dir)?;
        let git_marks = dir.join("git.marks");
        if !git_marks.exists() {
            std::fs::write(&git_marks, "")?;
        }
        Ok(Self {
            remote,
            flags,
            alias: safe,
            git_marks,
            our_marks: dir.join("sapwood.marks"),
            strict_push: std::env::var("SAPWOOD_PUSH").is_ok_and(|v| v == "strict"),
        })
    }

    fn private_ref(&self, branch: &str) -> String {
        format!("refs/sapwood/{}/heads/{branch}", self.alias)
    }

    /// Serve the remote-helper protocol until git closes the conversation.
    pub async fn serve(&self, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
        require(self.flags, Flag::GitBridge)?;
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let cmd = line.trim_end();
            match cmd {
                "" => return Ok(()),
                "capabilities" => {
                    writeln!(out, "import\nexport")?;
                    writeln!(out, "refspec refs/heads/*:refs/sapwood/{}/heads/*", self.alias)?;
                    writeln!(out, "*import-marks {}", self.git_marks.display())?;
                    writeln!(out, "*export-marks {}", self.git_marks.display())?;
                    writeln!(out, "option\n")?;
                }
                c if c == "list" || c.starts_with("list ") => {
                    let refs = self.remote.list_refs().await?;
                    for (name, _) in &refs {
                        writeln!(out, "? refs/heads/{name}")?;
                    }
                    if refs.iter().any(|(n, _)| n == "main") {
                        writeln!(out, "@refs/heads/main HEAD")?;
                    }
                    writeln!(out)?;
                }
                c if c.starts_with("option ") => writeln!(out, "unsupported")?,
                c if c.starts_with("import ") => {
                    let mut refs = vec![c["import ".len()..].to_owned()];
                    loop {
                        line.clear();
                        input.read_line(&mut line)?;
                        match line.trim_end().strip_prefix("import ") {
                            Some(r) => refs.push(r.to_owned()),
                            None => break, // the blank line ending the batch
                        }
                    }
                    self.import(&refs, out).await?;
                }
                "export" => self.export(input, out).await?,
                other => bail!("unsupported remote-helper command `{other}`"),
            }
            out.flush()?;
        }
    }

    /// Write a fast-import stream bringing each requested branch up to date.
    async fn import(&self, refs: &[String], out: &mut impl Write) -> Result<()> {
        let git = git_marks(&self.git_marks)?;
        let mut marks = Marks::load(&self.our_marks, &git)?;
        let mut next = git.keys().chain(marks.entries.keys()).max().copied().unwrap_or(0) + 1;
        let mut w = Writer { w: &mut *out };
        writeln!(w.w, "feature import-marks-if-exists={}", self.git_marks.display())?;
        writeln!(w.w, "feature export-marks={}", self.git_marks.display())?;
        writeln!(w.w, "feature done")?;
        // Private refs mirror the server. After a push the server rebased, the server's tip
        // is a sibling of the commit git recorded, so the mirror must move like a forced
        // fetch; the user's branches are never touched by this.
        writeln!(w.w, "feature force")?;
        let objects = self.remote.objects();
        for r in refs {
            let Some(branch) = r.strip_prefix("refs/heads/") else { continue };
            let Some(head) = self.remote.get_ref(branch).await? else { continue };
            // Newest-first walk until a changeset git already has.
            let mut chain = Vec::new();
            let mut cursor = Some(head);
            'walk: while let Some(rev) = cursor {
                if marks.commit_mark(&rev).is_some() {
                    break;
                }
                let page = self.remote.log(&rev.to_hex(), "", 1000).await?;
                cursor = None;
                for c in page {
                    if marks.commit_mark(&c.id).is_some() {
                        break 'walk;
                    }
                    cursor = c.changeset.first_parent()?;
                    chain.push(c);
                }
            }
            chain.reverse();
            let private = self.private_ref(branch);
            for rc in &chain {
                let cs = &rc.changeset;
                // On case-insensitive filesystems git's fast-import folds a case-only rename
                // (delete `A.txt`, write `a.txt`) onto the old name. For such commits emit
                // the complete tree after `deleteall`, which leaves nothing to fold.
                let case_rename = cs.changes.iter().any(|d| {
                    d.is_delete()
                        && cs
                            .changes
                            .iter()
                            .any(|u| !u.is_delete() && u.path != d.path && u.path.eq_ignore_ascii_case(&d.path))
                });
                let changes: Vec<FileChange> = if case_rename {
                    list_files(objects.as_ref(), rc.root_tree, &[])
                        .await?
                        .into_iter()
                        .map(|(p, (m, id))| FileChange::upsert(p, m, id))
                        .collect()
                } else {
                    cs.changes.clone()
                };
                let blob_ids: Vec<Id> = changes.iter().filter_map(|c| c.blob_id().ok().flatten()).collect();
                let payloads = objects.get_many(&blob_ids).await?;
                let mut contents: HashMap<Id, Vec<u8>> = HashMap::new();
                for (id, p) in blob_ids.iter().zip(payloads) {
                    let p = p.with_context(|| format!("blob {id} missing on server"))?;
                    contents.insert(*id, read_content(objects.as_ref(), id, &p).await?.1);
                }
                let ops: Vec<InlineOp> = changes
                    .iter()
                    .map(|c| {
                        Ok((
                            c.path.clone(),
                            match c.blob_id()? {
                                None => None,
                                Some(id) => Some((git_mode(Mode::parse(c.mode)?), contents[&id].clone())),
                            },
                        ))
                    })
                    .collect::<Result<_>>()?;
                let from = match cs.first_parent()? {
                    Some(p) => Some(marks.commit_mark(&p).with_context(|| format!("parent {p} unknown to git"))?),
                    None => None,
                };
                let mark = next;
                next += 1;
                w.commit_inline(
                    &private,
                    mark,
                    &cs.author,
                    cs.timestamp_ms.div_euclid(1000),
                    &cs.message,
                    from,
                    case_rename,
                    &ops,
                )?;
                marks.entries.insert(mark, ('c', rc.id));
            }
            let head_mark = marks.commit_mark(&head).context("branch head has no mark")?;
            writeln!(w.w, "reset {private}\nfrom :{head_mark}\n")?;
        }
        writeln!(w.w, "done")?;
        marks.save(&self.our_marks)?;
        Ok(())
    }

    /// Read git's fast-export stream, create changesets, and land each pushed branch.
    async fn export(&self, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
        let git = git_marks(&self.git_marks)?;
        let mut marks = Marks::load(&self.our_marks, &git)?;
        let mut blobs = marks.by_kind('b');
        let mut commits = marks.by_kind('c');
        let mut pending = Vec::new();
        let mut tips: BTreeMap<String, Id> = BTreeMap::new();
        let mut stats = ImportOutcome::default();
        let mut parser = Parser::new(&mut *input);
        while let Some(event) = parser.next_event()? {
            match event {
                Event::Blob { mark, data } => {
                    let (id, objects) = encode_blob(&data)?;
                    blobs.insert(mark, id);
                    marks.entries.insert(mark, ('b', id));
                    pending.extend(objects);
                }
                Event::Commit(c) => {
                    upload(self.remote, &std::mem::take(&mut pending)).await?;
                    let id = import_commit(self.remote, &c, &blobs, &commits, &mut stats).await?;
                    if let Some(m) = c.mark {
                        commits.insert(m, id);
                        marks.entries.insert(m, ('c', id));
                    }
                    tips.insert(c.refname.clone(), id);
                }
                Event::Reset { refname, from: Some(m) } => {
                    let id = *commits.get(&m).with_context(|| format!("unknown mark :{m} for {refname}"))?;
                    tips.insert(refname, id);
                }
                Event::Reset { .. } => {}
            }
        }
        drop(parser);
        upload(self.remote, &pending).await?;
        marks.save(&self.our_marks)?;
        for (refname, id) in tips {
            let Some(branch) = refname.strip_prefix("refs/heads/") else {
                writeln!(out, "error {refname} only branches can be pushed")?;
                continue;
            };
            match self.publish(branch, id).await {
                Ok(()) => writeln!(out, "ok {refname}")?,
                Err(e) => writeln!(out, "error {refname} {}", format!("{e:#}").replace('\n', " "))?,
            }
        }
        writeln!(out)?;
        Ok(())
    }

    async fn publish(&self, branch: &str, id: Id) -> Result<()> {
        if self.strict_push {
            let current = self.remote.get_ref(branch).await?;
            let parent = self.remote.changeset(&id.to_hex()).await?.changeset.first_parent()?;
            if current.is_some() && current != Some(id) && current != parent {
                bail!("non-fast-forward: fetch first (or unset SAPWOOD_PUSH=strict to let the server rebase)");
            }
            if !self.remote.compare_and_swap(branch, current, id).await? {
                bail!("branch moved during push; fetch and retry");
            }
            return Ok(());
        }
        let (landed, rebased) = self.remote.land(branch, id).await?;
        if rebased {
            eprintln!(
                "sapwood: {branch} had moved; the server rebased your push as {}. Run `git pull --rebase` to catch up.",
                landed.short()
            );
        }
        Ok(())
    }
}
