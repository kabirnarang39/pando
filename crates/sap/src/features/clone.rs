use std::path::Path;

use anyhow::{Result, bail};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::path::components;

use crate::remote::Remote;
use crate::workspace::{Config, Workspace};

/// Create a workspace in `dir` and materialise the ref head (only `config.sparse`
/// paths when given). Cloning a ref that does not exist yet yields an empty workspace.
pub async fn run(remote: &dyn Remote, flags: &dyn FeatureFlags, dir: &Path, config: Config) -> Result<Workspace> {
    if !config.sparse.is_empty() {
        require(flags, Flag::SparseCheckout)?;
    }
    for p in &config.sparse {
        components(p)?;
    }
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        bail!("{} exists and is not empty", dir.display());
    }
    std::fs::create_dir_all(dir)?;
    let mut ws = Workspace::init(dir, config)?;
    if let Some(head) = remote.get_ref(&ws.config.ref_name).await? {
        let cs = remote.changeset(&head.to_hex()).await?;
        ws.checkout(remote.objects(), cs.id, cs.root_tree).await?;
    }
    Ok(ws)
}
