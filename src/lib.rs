#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.MD")]

/// Verification, scanning, and rollback orchestration.
pub mod engine;
/// Basic filter transport boundary.
pub mod filter_source;
/// Validated block-header provider and chain snapshots.
pub mod headers;
/// Idempotent wallet callbacks.
pub mod hooks;
/// Bitcoin Core RPC adapter for a trusted full node.
#[cfg(feature = "rpc")]
pub mod rpc;
/// Durable block/filter commitments and scan progress.
pub mod store;

mod cfheaders;
mod matcher;

pub use engine::{Niebla158, SyncProgress};
pub use filter_source::{CfHeadersBatch, FilterSource};
pub use headers::{ChainTip, HeaderSource};
pub use hooks::WalletHooks;
pub use store::{FilterRecord, SqliteStore, Store};

/// Common integration types and all four required traits.
pub mod prelude {
    pub use crate::{
        CfHeadersBatch, ChainTip, FilterRecord, FilterSource, HeaderSource, Niebla158, SqliteStore,
        Store, SyncProgress, WalletHooks,
    };
}
