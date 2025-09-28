#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! niebla-158: a compact-filter (BIP-158) client engine for wallets.
//!
//! ## What you implement
//! - [`FilterSource`]: fetch cfheaders batches, per-block filters, and raw blocks.
//! - [`WalletHooks`]: provide a **watchlist** and handle **on_block_match** callbacks.
//! - [`Store`]: keep a couple of integers (verified tip + last scanned).
//!
//! ## What the engine does
//! - Validates **cfheaders** against optional checkpoints (defense-in-depth).
//! - Iterates new heights, pulls **filters**, tests against your watchlist.
//! - On a hit, fetches the **block**, decodes transactions, and notifies you.
//!
//! ## Minimal usage
//! ```rust,no_run
//! use niebla_158::prelude::*;
//! # struct MySource; # struct MyWallet;
//! # #[async_trait::async_trait] impl FilterSource for MySource {
//! #     async fn get_cfheaders(&self, _h: u32, _s: bitcoin::BlockHash) -> anyhow::Result<niebla_158::filter_source::CfHeaderBatch> { unimplemented!() }
//! #     async fn get_cfilter(&self, _b: bitcoin::BlockHash) -> anyhow::Result<Vec<u8>> { unimplemented!() }
//! #     async fn get_block(&self, _b: bitcoin::BlockHash) -> anyhow::Result<Vec<u8>> { unimplemented!() }
//! # }
//! # #[async_trait::async_trait] impl WalletHooks for MyWallet {
//! #     async fn watchlist(&self) -> anyhow::Result<Vec<niebla_158::hooks::WatchItem>> { Ok(vec![]) }
//! #     async fn on_block_match(&self, _h: u32, _bh: bitcoin::BlockHash, _txs: Vec<bitcoin::Transaction>) -> anyhow::Result<()> { Ok(()) }
//! # }
//! # async fn demo() -> anyhow::Result<()> {
//! let store = SqliteStore::new("niebla158.db")?;
//! let engine = Niebla158::new(store, MyWallet, MySource);
//! let headers = std::iter::empty::<(u32, bitcoin::BlockHash)>();
//! engine.run_to_tip(headers).await?;
//! # Ok(()) }

/// Engine that verifies cfheaders, scans filters, and fetches matching blocks.
pub mod engine;

/// Traits and types for fetching cfheaders, cfilters, and blocks from the network.
pub mod filter_source;

/// Wallet callbacks: provide a watchlist and receive matches.
pub mod hooks;

/// Persistence layer (traits and SQLite implementation).
pub mod store;

mod cfheaders;
mod checkpoints;
mod headers;
mod matcher;

pub use engine::Niebla158;
pub use filter_source::FilterSource;
pub use hooks::WalletHooks;
pub use store::Store;

pub use store::sqlite_store::SqliteStore;

/// Common re-exports for end users (engine + traits)
pub mod prelude {
    pub use crate::{FilterSource, Niebla158, Store, WalletHooks};
}

