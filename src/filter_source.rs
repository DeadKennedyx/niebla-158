//! Abstractions for fetching compact filter data from the network (HTTP or P2P).
use async_trait::async_trait;
use bitcoin::BlockHash;

/// A batch of rolling compact-filter headers returned by the source.
pub struct CfHeadersBatch {
    /// Height of the first header in `headers`.
    pub start_height: u32,
    /// Consecutive rolling cfheader hashes (each is a 32-byte array).
    pub headers: Vec<[u8; 32]>,
}

/// Network provider for compact-filter sync.
#[async_trait]
pub trait FilterSource: Send + Sync {
    /// Fetch a batch of rolling cfheaders starting at `start_h` and ending at the block `stop_hash`.
    async fn get_cfheaders(
        &self,
        start_h: u32,
        stop_hash: BlockHash,
    ) -> anyhow::Result<CfHeadersBatch>;

    /// Fetch the raw BIP-158 filter bytes for a given `block` hash.
    async fn get_cfilter(&self, block: BlockHash) -> anyhow::Result<Vec<u8>>;
    /// Fetch the raw consensus-encoded block bytes for `block` (used after a filter hit).
    async fn get_block(&self, block: BlockHash) -> anyhow::Result<Vec<u8>>;
}
