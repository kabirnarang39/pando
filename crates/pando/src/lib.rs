//! Pando client library. `main.rs` is a thin CLI over the feature slices here.

pub mod bridge;
pub mod features;
pub mod gitfe;
pub mod mcp;
pub mod mount;
pub mod remote;
pub mod workspace;

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `$PANDO_AUTHOR`, else `git config user.name/email`, else `$USER`.
pub fn default_author() -> String {
    if let Ok(a) = std::env::var("PANDO_AUTHOR") {
        return a;
    }
    let git = |k: &str| {
        std::process::Command::new("git")
            .args(["config", "--get", k])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    match (git("user.name"), git("user.email")) {
        (Some(n), Some(e)) => format!("{n} <{e}>"),
        (Some(n), None) => n,
        _ => std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
    }
}
