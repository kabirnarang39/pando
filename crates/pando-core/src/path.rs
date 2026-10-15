//! Repository path rules. Paths cross trust boundaries (client -> server, server ->
//! client filesystem), so every component is validated before use.

use crate::{Error, Result};

/// Directory reserved for client metadata; never tracked.
pub const META_DIR: &str = ".pando";

pub fn validate_name(name: &str) -> Result<()> {
    let bad = name.is_empty() || name == "." || name == ".." || name == META_DIR || name.contains(['/', '\\', '\0']);
    if bad { Err(Error::invalid(format!("invalid path component `{name}`"))) } else { Ok(()) }
}

/// Split a repo-relative path (`a/b/c`) into validated components.
pub fn components(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() {
        return Err(Error::invalid("empty path"));
    }
    let parts: Vec<&str> = path.split('/').collect();
    for p in &parts {
        validate_name(p).map_err(|_| Error::invalid(format!("invalid path `{path}`")))?;
    }
    Ok(parts)
}

/// `""` denotes the root; otherwise same rules as [`components`].
pub fn components_or_root(path: &str) -> Result<Vec<&str>> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() { Ok(Vec::new()) } else { components(trimmed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_traversal_and_reserved() {
        for bad in ["", "/a", "a/", "a//b", "../x", "a/../b", "./a", ".pando/x", "a\\b", "a\0b"] {
            assert!(components(bad).is_err(), "{bad:?} accepted");
        }
        assert_eq!(components("src/main.rs").unwrap(), ["src", "main.rs"]);
        assert!(components_or_root("/").unwrap().is_empty());
    }

    proptest! {
        // Any accepted path joined under a root stays under that root.
        #[test]
        fn accepted_paths_never_escape(path in "[a-z./\\\\]{0,24}") {
            if let Ok(parts) = components(&path) {
                let joined = std::path::Path::new("/root").join(parts.join("/"));
                prop_assert!(joined.starts_with("/root"));
                prop_assert!(!parts.iter().any(|p| *p == ".." || *p == "."));
            }
        }
    }
}
