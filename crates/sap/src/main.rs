use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sap::features::{browse, clone, commit, diff, git, land, sync};
use sap::remote::{GrpcRemote, Remote};
use sap::workspace::{ChangeKind, Config, Workspace};
use sapwood_adapters::flags::SpecFlags;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "sap", version, about = "Sapwood: source control for monorepos at agent scale", propagate_version = true)]
struct Cli {
    /// Bearer token for the server ($SAP_TOKEN).
    #[arg(long, global = true, env = "SAP_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a workspace and check out a ref (lazily, and sparsely with --path).
    Clone {
        server: String,
        dir: PathBuf,
        #[arg(long = "ref", default_value = "main")]
        ref_name: String,
        /// Only materialise these paths (repeatable).
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Fetch objects through a node daemon or regional cache.
        #[arg(long, env = "SAP_OBJECTS")]
        objects: Option<String>,
    },
    /// Show working-copy changes.
    #[command(visible_alias = "st")]
    Status { paths: Vec<String> },
    /// Create a changeset on the server from working-copy changes (all, or PATHS).
    #[command(visible_alias = "ci")]
    Commit {
        #[arg(short, long)]
        message: String,
        /// Accepted for git muscle memory; Sapwood always commits all changes (or PATHS).
        #[arg(short = 'a', long = "all", hide = true)]
        _all: bool,
        paths: Vec<String>,
    },
    /// Land the committed work on the ref (server-side rebase + atomic ref update).
    #[command(visible_alias = "push")]
    Land,
    /// Update the working copy to the ref head (or REV).
    #[command(visible_alias = "pull")]
    Sync { rev: Option<String> },
    /// Switch to a ref or changeset; -b creates a ref at the current base.
    #[command(visible_alias = "switch")]
    Checkout {
        target: String,
        #[arg(short = 'b')]
        create: bool,
    },
    /// Changeset history (server-side; no local history needed).
    Log {
        /// Limit to a path.
        path: Option<String>,
        #[arg(long)]
        rev: Option<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        oneline: bool,
    },
    /// Diff the working copy, or two revisions (`sap diff A B`).
    Diff {
        revs: Vec<String>,
        #[arg(long = "name-only")]
        names_only: bool,
    },
    /// Show a changeset and its patch against its first parent.
    Show { rev: Option<String> },
    /// Print a file at a revision without checking anything out.
    Cat { rev: String, path: String },
    /// List a directory at a revision without checking anything out.
    Ls {
        rev: String,
        #[arg(default_value = "")]
        path: String,
    },
    /// Find files by path substring at a revision (server-side).
    Find {
        pattern: String,
        #[arg(long, default_value = "main")]
        rev: String,
    },
    /// Show which changeset last changed each line of a file.
    Blame {
        path: String,
        #[arg(long, default_value = "main")]
        rev: String,
    },
    /// List refs.
    #[command(visible_alias = "branch")]
    Refs,
    /// Best common ancestor of two revisions.
    MergeBase { a: String, b: String },
    /// No-op: Sapwood has no staging area. Use `sap commit [PATHS]`.
    #[command(hide = true)]
    Add { paths: Vec<String> },
    /// Import a git repository's history.
    ImportGit {
        git_dir: PathBuf,
        #[arg(long)]
        server: Option<String>,
        #[arg(long = "git-ref", default_value = "HEAD")]
        git_ref: String,
        #[arg(long = "ref", default_value = "main")]
        ref_name: String,
        #[arg(long)]
        force: bool,
    },
    /// Serve Sapwood as MCP tools over stdio for AI agents (Claude Code, Codex, Cursor...).
    Mcp {
        #[arg(long)]
        server: Option<String>,
    },
    /// Export first-parent history to a git repository.
    ExportGit {
        git_dir: PathBuf,
        #[arg(long)]
        server: Option<String>,
        #[arg(long, default_value = "main")]
        rev: String,
        #[arg(long = "git-ref", default_value = "main")]
        git_ref: String,
    },
}

fn workspace() -> Result<Workspace> {
    Workspace::find(Path::new("."))
}

fn connect(server: &str, objects: Option<&str>, token: Option<&str>) -> Result<Arc<dyn Remote>> {
    Ok(Arc::new(GrpcRemote::connect(server, objects, token)?))
}

fn ws_remote(ws: &Workspace, token: Option<&str>) -> Result<Arc<dyn Remote>> {
    connect(&ws.config.server, ws.config.objects.as_deref(), token)
}

/// Server for workspace-free commands: --server, $SAP_SERVER, or the current workspace.
fn server_for(explicit: Option<String>) -> Result<String> {
    explicit
        .or_else(|| std::env::var("SAP_SERVER").ok())
        .or_else(|| workspace().ok().map(|w| w.config.server))
        .context("no server: pass --server, set SAP_SERVER, or run inside a workspace")
}

fn short_message(m: &str) -> &str {
    m.lines().next().unwrap_or("")
}

fn fmt_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant), UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", tod / 3600, tod % 3600 / 60)
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let flags = SpecFlags::from_env()?;
    let token = cli.token.as_deref();
    let mut stdout = std::io::stdout().lock();
    match cli.cmd {
        Cmd::Clone { server, dir, ref_name, paths, objects } => {
            let remote = connect(&server, objects.as_deref(), token)?;
            let config = Config { server, objects, ref_name, sparse: paths };
            let ws = clone::run(remote.as_ref(), &flags, &dir, config).await?;
            match ws.base {
                Some(b) => println!("checked out {} files at {} into {}", ws.index.len(), b.short(), dir.display()),
                None => println!("cloned empty ref into {}", dir.display()),
            }
        }
        Cmd::Status { paths } => {
            let ws = workspace()?;
            let changes = ws.status(&paths)?;
            for c in &changes {
                let tag = match c.kind {
                    ChangeKind::Added => "A",
                    ChangeKind::Modified => "M",
                    ChangeKind::Deleted => "D",
                };
                writeln!(stdout, "{tag} {}", c.path)?;
            }
            if changes.is_empty() {
                writeln!(stdout, "clean (base {})", ws.base.map_or("none".into(), |b| b.short()))?;
            }
        }
        Cmd::Commit { message, paths, .. } => {
            let mut ws = workspace()?;
            let remote = ws_remote(&ws, token)?;
            match commit::run(remote.as_ref(), &mut ws, &message, &sap::default_author(), &paths).await? {
                Some(c) => println!("committed {} ({} files); run `sap land` to land on {}", c.id.short(), c.files, ws.config.ref_name),
                None => println!("nothing to commit"),
            }
        }
        Cmd::Land => {
            let mut ws = workspace()?;
            let remote = ws_remote(&ws, token)?;
            let out = land::run(remote.as_ref(), &mut ws).await?;
            println!(
                "landed {} on {}{}",
                out.landed.short(),
                ws.config.ref_name,
                if out.rebased { format!(" (rebased; {} incoming files synced)", out.synced) } else { String::new() }
            );
            if let Some(why) = out.behind {
                eprintln!("warning: working copy not updated: {why}");
            }
        }
        Cmd::Sync { rev } => {
            let mut ws = workspace()?;
            let remote = ws_remote(&ws, token)?;
            let out = sync::run(remote.as_ref(), &mut ws, rev.as_deref()).await?;
            println!("at {} ({} files changed)", out.to.short(), out.changed);
        }
        Cmd::Checkout { target, create } => {
            let mut ws = workspace()?;
            let remote = ws_remote(&ws, token)?;
            if create {
                sync::create_branch(remote.as_ref(), &mut ws, &target).await?;
                println!("created ref {target} at {}", ws.base.map_or("none".into(), |b| b.short()));
            } else {
                let out = sync::checkout(remote.as_ref(), &mut ws, &target).await?;
                println!("at {} ({} files changed)", out.to.short(), out.changed);
            }
        }
        Cmd::Log { path, rev, limit, oneline } => {
            let (remote, rev) = match workspace() {
                Ok(ws) => {
                    let rev = rev.or(ws.base.map(|b| b.to_hex())).unwrap_or(ws.config.ref_name.clone());
                    (ws_remote(&ws, token)?, rev)
                }
                Err(_) => (connect(&server_for(None)?, None, token)?, rev.unwrap_or("main".into())),
            };
            for c in browse::log(remote.as_ref(), &rev, path.as_deref().unwrap_or(""), limit).await? {
                if oneline {
                    writeln!(stdout, "{} {}", c.id.short(), short_message(&c.changeset.message))?;
                } else {
                    writeln!(stdout, "changeset {}", c.id)?;
                    writeln!(stdout, "Author: {}", c.changeset.author)?;
                    writeln!(stdout, "Date:   {}\n", fmt_time(c.changeset.timestamp_ms))?;
                    for l in c.changeset.message.trim_end().lines() {
                        writeln!(stdout, "    {l}")?;
                    }
                    writeln!(stdout)?;
                }
            }
        }
        Cmd::Diff { revs, names_only } => {
            let text = match revs.as_slice() {
                [] => {
                    let ws = workspace()?;
                    diff::working(ws_remote(&ws, token)?.as_ref(), &ws, &[], names_only).await?
                }
                [a, b] => diff::revisions(connect(&server_for(None)?, None, token)?.as_ref(), a, b, names_only).await?,
                _ => anyhow::bail!("usage: sap diff [FROM TO]"),
            };
            stdout.write_all(text.as_bytes())?;
        }
        Cmd::Show { rev } => {
            let ws = workspace().ok();
            let remote = match &ws {
                Some(w) => ws_remote(w, token)?,
                None => connect(&server_for(None)?, None, token)?,
            };
            let rev = rev.or(ws.and_then(|w| w.base.map(|b| b.to_hex()))).context("no revision")?;
            let c = remote.changeset(&rev).await?;
            writeln!(stdout, "changeset {}\nAuthor: {}\nDate:   {}\n", c.id, c.changeset.author, fmt_time(c.changeset.timestamp_ms))?;
            for l in c.changeset.message.trim_end().lines() {
                writeln!(stdout, "    {l}")?;
            }
            writeln!(stdout)?;
            if let Some(p) = c.changeset.first_parent()? {
                stdout.write_all(diff::revisions(remote.as_ref(), &p.to_hex(), &c.id.to_hex(), false).await?.as_bytes())?;
            }
        }
        Cmd::Cat { rev, path } => {
            let remote = connect(&server_for(None)?, None, token)?;
            stdout.write_all(&browse::cat(remote.as_ref(), &rev, &path).await?)?;
        }
        Cmd::Ls { rev, path } => {
            let remote = connect(&server_for(None)?, None, token)?;
            for e in browse::ls(remote.as_ref(), &rev, &path).await? {
                writeln!(stdout, "{} {} {}", browse::mode_char(e.mode), e.id.short(), e.name)?;
            }
        }
        Cmd::Find { pattern, rev } => {
            let remote = connect(&server_for(None)?, None, token)?;
            for p in remote.find(&rev, &pattern, 10_000).await? {
                writeln!(stdout, "{p}")?;
            }
        }
        Cmd::Blame { path, rev } => {
            let remote = connect(&server_for(None)?, None, token)?;
            for (i, (id, line)) in remote.blame(&rev, &path).await?.into_iter().enumerate() {
                writeln!(stdout, "{} {:>5} {line}", id.short(), i + 1)?;
            }
        }
        Cmd::Refs => {
            let remote = connect(&server_for(None)?, None, token)?;
            let current = workspace().ok().map(|w| w.config.ref_name);
            for (name, id) in remote.list_refs().await? {
                let mark = if current.as_deref() == Some(&name) { "*" } else { " " };
                writeln!(stdout, "{mark} {name} {}", id.short())?;
            }
        }
        Cmd::MergeBase { a, b } => {
            let remote = connect(&server_for(None)?, None, token)?;
            match remote.merge_base(&a, &b).await? {
                Some(id) => writeln!(stdout, "{id}")?,
                None => anyhow::bail!("no common ancestor"),
            }
        }
        Cmd::Add { .. } => {
            println!("sapwood has no staging area: `sap commit -m MSG [PATHS]` commits all changes (or just PATHS)");
        }
        Cmd::ImportGit { git_dir, server, git_ref, ref_name, force } => {
            let remote = connect(&server_for(server)?, None, token)?;
            let started = std::time::Instant::now();
            let out = git::import(remote.as_ref(), &flags, &git_dir, &git_ref, &ref_name, force).await?;
            println!(
                "imported {} commits, {} blobs ({} uploaded) in {:.1}s; {ref_name} -> {}{}",
                out.commits,
                out.blobs,
                out.uploaded,
                started.elapsed().as_secs_f64(),
                out.tip.map_or("none".into(), |t| t.short()),
                if out.skipped_submodules > 0 { format!(" ({} submodule entries skipped)", out.skipped_submodules) } else { String::new() }
            );
        }
        Cmd::Mcp { server } => {
            let ws = workspace().ok();
            let remote = match &ws {
                Some(w) if server.is_none() => ws_remote(w, token)?,
                _ => connect(&server_for(server)?, None, token)?,
            };
            drop(stdout);
            sap::mcp::SapMcp::new(remote, ws.map(|w| w.root)).serve_stdio().await?;
        }
        Cmd::ExportGit { git_dir, server, rev, git_ref } => {
            let remote = connect(&server_for(server)?, None, token)?;
            let n = git::export(remote.as_ref(), &flags, &rev, &git_dir, &git_ref).await?;
            println!("exported {n} commits to {} ({git_ref})", git_dir.display());
        }
    }
    Ok(())
}
