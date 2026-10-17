//! `pandod backup`: a consistent copy of a platform server's state (ADR-030). The registry
//! (accounts, tokens, change requests, issues...) and every hosted repository's metadata
//! database are each snapshotted with SQLite's `VACUUM INTO`, which reads one transaction, so
//! it is safe while the server runs. Objects kept apart from metadata (`--repos-objects`) are
//! content-addressed and only ever added; the manifest records where they live, and the
//! operator backs that store up with the provider's own tools (bucket versioning, replication).
//!
//! Restoring is copying back while the server is stopped; `docs/deploy.md` gives the steps.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::config::redact;
use crate::hosting::{Registry, Templates, now_ms};

/// Name of the registry snapshot and the manifest inside a backup.
pub const REGISTRY_FILE: &str = "platform.db";
pub const MANIFEST_FILE: &str = "manifest.json";

/// What a backup holds.
#[derive(Debug)]
pub struct Summary {
    pub repositories: usize,
    pub files: Vec<String>,
}

fn sqlite_path<'a>(url: &'a str, what: &str) -> anyhow::Result<&'a str> {
    url.strip_prefix("sqlite://").with_context(|| {
        format!("{what} `{}` is not SQLite; back it up with its own tools (e.g. pg_dump)", redact(url))
    })
}

/// A transactionally consistent copy of the SQLite database at `src` in `dst`.
fn snapshot(src: &str, dst: &Path) -> anyhow::Result<()> {
    let db = rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {src}"))?;
    db.execute("VACUUM INTO ?1", [dst.to_str().context("backup path is not UTF-8")?])
        .with_context(|| format!("snapshotting {src}"))?;
    Ok(())
}

/// Backs up the registry at `platform`, every repository it lists (storage from `templates`)
/// and, if given, the server's default repository, to `to`: an empty or new directory, or an
/// object store URL (`s3://bucket/prefix`, `gs://...`, `az://...`).
pub async fn backup(
    platform: &str,
    templates: &Templates,
    default_metadata: Option<&str>,
    to: &str,
) -> anyhow::Result<Summary> {
    let remote = to.contains("://");
    let staging = if remote { Some(tempfile::tempdir()?) } else { None };
    let dir: PathBuf = match &staging {
        Some(t) => t.path().to_owned(),
        None => PathBuf::from(to),
    };
    std::fs::create_dir_all(dir.join("repos")).with_context(|| format!("creating {}", dir.display()))?;
    if dir.join(MANIFEST_FILE).exists() || dir.join(REGISTRY_FILE).exists() {
        bail!("{} already holds a backup; give a new directory", dir.display());
    }

    // The registry first: the repositories it lists are the ones backed up.
    snapshot(sqlite_path(platform, "--platform")?, &dir.join(REGISTRY_FILE))?;
    let listed = Registry::open(&format!("sqlite://{}", dir.join(REGISTRY_FILE).display()))?.list(None)?;
    let mut files = vec![REGISTRY_FILE.to_owned()];
    let mut repos = Vec::new();
    for rec in &listed {
        let (metadata, objects) = templates.render(&rec.key);
        let src = sqlite_path(&metadata, &format!("repository {}'s metadata", rec.key))?;
        let file = format!("repos/{}.db", rec.key.id());
        // A repository never opened has no database yet: nothing to copy.
        let copied = Path::new(src).exists();
        if copied {
            snapshot(src, &dir.join(&file))?;
            files.push(file.clone());
        }
        repos.push(serde_json::json!({
            "repository": rec.key.to_string(),
            "metadata": redact(&metadata),
            "objects": redact(&objects),
            "file": if copied { file } else { String::new() },
        }));
    }
    let mut default = serde_json::Value::Null;
    if let Some(m) = default_metadata
        && let Some(src) = m.strip_prefix("sqlite://")
        && Path::new(src).exists()
    {
        snapshot(src, &dir.join("default.db"))?;
        files.push("default.db".to_owned());
        default = serde_json::json!({ "metadata": redact(m), "file": "default.db" });
    }
    let manifest = serde_json::json!({
        "created_at_ms": now_ms(),
        "pandod": env!("CARGO_PKG_VERSION"),
        "platform": redact(platform),
        "registry_file": REGISTRY_FILE,
        "repos_metadata": redact(&templates.metadata),
        "repos_objects": redact(&templates.objects),
        "objects_note": "objects outside `metadata` are not copied: back that store up with its provider's tools",
        "default": default,
        "repositories": repos,
    });
    std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec_pretty(&manifest)?)?;
    files.push(MANIFEST_FILE.to_owned());

    if remote {
        upload(&dir, &files, to).await?;
    }
    Ok(Summary { repositories: listed.len(), files })
}

/// Puts each backed-up file under the object store URL `to`, in a folder named by the time.
async fn upload(dir: &Path, files: &[String], to: &str) -> anyhow::Result<()> {
    use object_store::ObjectStoreExt;
    let url = url::Url::parse(to).with_context(|| format!("`{to}` is not a directory or a URL"))?;
    let env = std::env::vars().map(|(k, v)| (k.to_ascii_lowercase(), v));
    let (store, prefix) = object_store::parse_url_opts(&url, env)?;
    let folder = prefix.clone().join(format!("pando-backup-{}", now_ms()));
    for f in files {
        let mut at = folder.clone();
        for part in f.split('/') {
            at = at.join(part);
        }
        let bytes = std::fs::read(dir.join(f))?;
        store.put(&at, bytes.into()).await.with_context(|| format!("uploading {f}"))?;
    }
    Ok(())
}
