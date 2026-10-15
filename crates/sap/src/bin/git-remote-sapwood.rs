//! git remote helper: `git clone sapwood::http://host:7420 repo`.
//!
//! git invokes `git-remote-sapwood <remote> <url>` with `GIT_DIR` set and speaks the
//! remote-helper protocol on stdin/stdout. All logic lives in `sap::bridge`.

use std::io::{BufWriter, stdin, stdout};
use std::path::PathBuf;

use anyhow::Context;
use sap::bridge::Bridge;
use sap::remote::GrpcRemote;
use sapwood_adapters::flags::SpecFlags;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("git-remote-sapwood: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().context("usage: git-remote-sapwood <remote> <url>")?;
    let url = args.next().unwrap_or_else(|| name.clone());
    let url = url.strip_prefix("sapwood::").unwrap_or(&url).to_owned();
    // A bare URL (no configured remote) gets a generic alias, as git's own helpers do.
    let alias = if name.contains("://") { "_".to_owned() } else { name };
    let git_dir = PathBuf::from(std::env::var("GIT_DIR").context("GIT_DIR is not set; run through git")?);
    let token = std::env::var("SAP_TOKEN").ok();
    let objects = std::env::var("SAP_OBJECTS").ok();
    let remote = GrpcRemote::connect(&url, objects.as_deref(), token.as_deref())?;
    let flags = SpecFlags::from_env()?;
    let bridge = Bridge::new(&remote, &flags, &alias, &git_dir)?;
    let mut input = stdin().lock();
    let mut out = BufWriter::new(stdout().lock());
    bridge.serve(&mut input, &mut out).await
}
