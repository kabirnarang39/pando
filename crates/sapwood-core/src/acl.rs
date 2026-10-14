//! Access policy port: per-directory write permissions and per-ref update permissions.
//!
//! Read access is repository-wide in v1: objects are content-addressed and reachable
//! through parent trees, so path-scoped reads need object-level scoping (roadmap).

use crate::{Error, Result};

/// Authenticated identity of the caller. `anonymous` when auth is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal(pub String);

impl Principal {
    pub fn anonymous() -> Self {
        Self("anonymous".into())
    }
}

pub trait AccessPolicy: Send + Sync {
    fn can_write(&self, who: &Principal, path: &str) -> bool;
    fn can_update_ref(&self, who: &Principal, ref_name: &str) -> bool;
}

/// Allows everything.
pub struct AllowAllPolicy;
impl AccessPolicy for AllowAllPolicy {
    fn can_write(&self, _: &Principal, _: &str) -> bool {
        true
    }
    fn can_update_ref(&self, _: &Principal, _: &str) -> bool {
        true
    }
}

pub fn require_write<'a>(policy: &dyn AccessPolicy, who: &Principal, paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let denied: Vec<&str> = paths.into_iter().filter(|p| !policy.can_write(who, p)).take(10).collect();
    if denied.is_empty() {
        Ok(())
    } else {
        Err(Error::PermissionDenied(format!("{} may not write {}", who.0, denied.join(", "))))
    }
}

pub fn require_ref(policy: &dyn AccessPolicy, who: &Principal, ref_name: &str) -> Result<()> {
    if policy.can_update_ref(who, ref_name) {
        Ok(())
    } else {
        Err(Error::PermissionDenied(format!("{} may not update ref `{ref_name}`", who.0)))
    }
}
