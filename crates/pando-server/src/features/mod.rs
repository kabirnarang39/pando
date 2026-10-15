//! Vertical feature slices. Each slice owns its use case, its gRPC adapter and its
//! feature-flag check; slices share only the domain kernel (`pando_core::Repo`).

pub mod access;
pub mod browse;
pub mod changesets;
pub mod compact;
pub mod diff;
pub mod history;
pub mod land;
pub mod merge_base;
pub mod objects;
pub mod prefetch;
pub mod refs;
