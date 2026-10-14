//! Infrastructure adapters for the ports in `sapwood_core::ports`.
//!
//! | Port            | Adapters                                                        |
//! |-----------------|-----------------------------------------------------------------|
//! | `ObjectStore`   | memory, SQLite, Postgres, object storage (S3/GCS/Azure/local), bounded cache, tiered read-through, gRPC upstream |
//! | `ChangesetLog`  | memory, SQLite, Postgres                                        |
//! | `RefStore`      | memory, SQLite, Postgres                                        |
//! | `DerivedStore`  | memory, SQLite, Postgres                                        |
//! | `FeatureFlags`  | spec string / env (`SAPWOOD_FLAGS`)                             |

pub mod cache;
pub mod cloud;
pub mod flags;
pub mod grpc;
pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub(crate) fn storage_err(e: impl std::fmt::Display) -> sapwood_core::Error {
    sapwood_core::Error::Storage(anyhow::anyhow!("{e}"))
}
