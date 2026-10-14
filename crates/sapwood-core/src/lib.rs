//! Sapwood domain kernel.
//!
//! Everything here is pure domain logic over the ports in [`ports`]; no transport,
//! database or cloud SDK is referenced. Infrastructure lives in `sapwood-adapters`.

pub mod acl;
mod derived;
mod error;
pub mod flags;
mod id;
pub mod object;
pub mod path;
pub mod ports;
mod repo;
pub mod tree;

pub use derived::{Derivation, Generation, RootTree};
pub use error::{Error, Result};
pub use id::Id;
pub use derived::GraphInfo;
pub use repo::{Repo, paths_overlap, validate_ref_name};
