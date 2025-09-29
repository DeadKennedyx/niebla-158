#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! niebla-158: a compact-filter (BIP-158) client engine for wallets.
//!
//! ## What you implement
//! - [`FilterSource`]: fetch cfheaders batches, per-block filters, and raw blocks.
//! - [`WalletHooks`]: provide a **watchlist** and handle **on_block_match** callbacks.
//! - [`Store`]: keep a couple of integers (verified tip + last scanned).
//! - [`HeaderSource`]: return block header info by height (used to scan ranges).
//!
//! ## What the engine does
//! - Validates **cfheaders** against optional checkpoints (defense-in-depth).
//! - Iterates new heights, pulls **filters**, tests against your watchlist.
//! - On a hit, fetches the **block**, decodes transactions, and notifies you.
//!
//! ## Minimal usage
//! ```rust,ignore
//! use niebla_158::prelude::*;
//! use bitcoin::{BlockHash, ScriptBuf, hashes::{sha256d, Hash as _}};
//! use async_trait::async_trait;
//!
//! // --- Your implementations ---
//! struct MySource;
//! #[async_trait]
//! impl FilterSource for MySource {
//!     async fn get_cfheaders(&self, _start: u32, _stop: BlockHash) -> anyhow::Result<CfHeadersBatch> {
//!         Ok(CfHeadersBatch { start_height: 0, headers: vec![] })
//!     }
//!     async fn get_cfilter(&self, _block: BlockHash) -> anyhow::Result<Vec<u8>> { Ok(vec![]) }
//!     async fn get_block(&self, _block: BlockHash) -> anyhow::Result<Vec<u8>> { Ok(vec![]) }
//! }
//!
//! struct MyHeaders;
//! #[async_trait]
//! impl HeaderSource for MyHeaders {
//!     async fn tip_height(&self) -> anyhow::Result<u32> { Ok(0) }
//!     async fn hash_at_height(&self, _h: u32) -> anyhow::Result<BlockHash> {
//!         Ok(BlockHash::from_raw_hash(sha256d::Hash::all_zeros()))
//!     }
//! }
//!
//! struct MyStore;
//! #[async_trait]
//! impl Store for MyStore {
//!     async fn load_cf_tip(&self) -> anyhow::Result<Option<(u32, BlockHash)>> { Ok(None) }
//!     async fn save_cf_tip(&self, _h: u32, _cf: BlockHash) -> anyhow::Result<()> { Ok(()) }
//!     async fn get_last_scanned(&self) -> anyhow::Result<u32> { Ok(0) }
//!     async fn set_last_scanned(&self, _h: u32) -> anyhow::Result<()> { Ok(()) }
//!     async fn get_birth_height(&self) -> anyhow::Result<Option<u32>> { Ok(None) }
//!     async fn set_birth_height(&self, _h: u32) -> anyhow::Result<()> { Ok(()) }
//! }
//!
//! struct MyWallet;
//! #[async_trait]
//! impl WalletHooks for MyWallet {
//!     async fn watchlist(&self) -> anyhow::Result<Vec<ScriptBuf>> { Ok(vec![]) }
//!     async fn on_block_match(
//!         &self, _h: u32, _b: BlockHash, _txs: Vec<bitcoin::Transaction>
//!     ) -> anyhow::Result<()> { Ok(()) }
//! }
//!
//! // --- Wire it up ---
//! async fn run() -> anyhow::Result<()> {
//!     let engine = Niebla158::new(MyStore, MyWallet, MySource, MyHeaders);
//!     // Drive with an iterator of (height, header_hash); here empty:
//!     engine.run_to_tip(std::iter::empty()).await?;
//!     Ok(())
//! }
//! ```
/// Engine that verifies cfheaders, scans filters, and fetches matching blocks.
pub mod engine;

/// Traits and types for fetching cfheaders, cfilters, and blocks from the network.
pub mod filter_source;

/// Wallet callbacks: provide a watchlist and receive matches.
pub mod hooks;

/// Block header lookup abstraction (height → hash).
pub mod headers;

// Internal helpers:
mod cfheaders;
mod checkpoints;
mod matcher;

/// Persistence layer (traits and SQLite implementation).
pub mod store;

// Public re-exports
pub use engine::Niebla158;
pub use filter_source::FilterSource;
pub use hooks::WalletHooks;
pub use store::{sqlite_store::SqliteStore, Store};

/// Convenience prelude for end users.
pub mod prelude {
    pub use crate::{FilterSource, Niebla158, SqliteStore, Store, WalletHooks};
}

