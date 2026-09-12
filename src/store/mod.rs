//! Durable filter commitments and wallet scan progress.
use async_trait::async_trait;
use bitcoin::{
    bip158::{FilterHash, FilterHeader},
    BlockHash,
};

/// Commitment to one block's basic filter on the synchronized chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterRecord {
    /// Block height, starting at zero.
    pub height: u32,
    /// Identity of the associated Bitcoin block.
    pub block_hash: BlockHash,
    /// Double-SHA256 of the serialized filter bytes.
    pub filter_hash: FilterHash,
    /// Rolling BIP-157 header after this filter.
    pub filter_header: FilterHeader,
}

/// Storage contract for one engine/wallet pair.
///
/// Retain contiguous commitments from genesis. Batch appends and chain rewinds
/// must be atomic, including updates to scan progress during a rewind. Use one
/// engine per store; the engine serializes its own sync and rescan operations.
#[async_trait]
pub trait Store: Send + Sync {
    /// Last stored commitment, or `None` before genesis is synchronized.
    async fn load_cf_tip(&self) -> anyhow::Result<Option<FilterRecord>>;
    /// Commitment at a previously synchronized height.
    async fn load_filter(&self, height: u32) -> anyhow::Result<Option<FilterRecord>>;
    /// Atomically append a nonempty contiguous batch and update the tip.
    /// Implementations must reject gaps, overwrites and invalid rolling links.
    async fn save_filters(&self, records: &[FilterRecord]) -> anyhow::Result<()>;
    /// Atomically remove commitments and scan progress above this height.
    /// `None` clears all commitments and scan progress; preserve birth height.
    async fn rewind(&self, retained_height: Option<u32>) -> anyhow::Result<()>;
    /// Last scanned height. `None` distinguishes an unscanned genesis block.
    async fn get_last_scanned(&self) -> anyhow::Result<Option<u32>>;
    /// Set scan progress, rejecting heights without stored commitments.
    /// `None` resets the cursor, without removing filter commitments.
    async fn set_last_scanned(&self, height: Option<u32>) -> anyhow::Result<()>;
    /// Optional first height to scan when no explicit rescan was requested.
    async fn get_birth_height(&self) -> anyhow::Result<Option<u32>>;
    /// Set or clear the wallet's birth height. Zero is a valid explicit value.
    async fn set_birth_height(&self, height: Option<u32>) -> anyhow::Result<()>;
}

/// SQLite implementation with a connection retained for its entire lifetime.
pub mod sqlite_store;
pub use sqlite_store::SqliteStore;
