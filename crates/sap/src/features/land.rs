use anyhow::{Result, bail};
use sapwood_core::Id;

use super::guard_dirty;
use crate::remote::Remote;
use crate::workspace::Workspace;

#[derive(Debug)]
pub struct LandOutcome {
    pub landed: Id,
    pub rebased: bool,
    /// Files updated in the working copy from changes others landed first.
    pub synced: usize,
    /// Set when the working copy could not be updated (uncommitted edits overlap).
    pub behind: Option<String>,
}

/// Land the workspace base on its ref. The server rebases if the ref moved; incoming
/// changes are then applied locally unless they clash with uncommitted edits.
pub async fn run(remote: &dyn Remote, ws: &mut Workspace) -> Result<LandOutcome> {
    let Some(base) = ws.base else { bail!("nothing to land: commit first") };
    let (landed, rebased) = remote.land(&ws.config.ref_name, base).await?;
    ws.base_landed = true;
    let mut out = LandOutcome { landed, rebased, synced: 0, behind: None };
    if landed != base {
        let incoming = remote.diff(&base.to_hex(), &landed.to_hex()).await?;
        match guard_dirty(ws, &incoming) {
            Ok(()) => {
                ws.apply(remote.objects(), &incoming).await?;
                ws.base = Some(landed);
                out.synced = incoming.len();
            }
            Err(e) => out.behind = Some(e.to_string()),
        }
    }
    ws.save()?;
    Ok(out)
}
