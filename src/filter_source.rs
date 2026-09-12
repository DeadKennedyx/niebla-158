//! Boundary for basic BIP-157 filters and raw Bitcoin blocks.
use async_trait::async_trait;
use bitcoin::{
    bip158::{FilterHash, FilterHeader},
    BlockHash,
};

/// A BIP-157 `cfheaders` response containing filter hashes, not rolling headers.
#[derive(Clone, Debug)]
pub struct CfHeadersBatch {
    /// BIP-158 basic filter type: zero.
    pub filter_type: u8,
    /// First requested height.
    pub start_height: u32,
    /// Last requested block hash.
    pub stop_hash: BlockHash,
    /// Rolling header immediately before `start_height`; zero before genesis.
    pub previous_filter_header: FilterHeader,
    /// One filter hash per requested block, in ascending height order.
    pub filter_hashes: Vec<FilterHash>,
}

/// Fetch filter data without disclosing the wallet's script watchlist.
///
/// Returning hashes from one untrusted peer does not authenticate them. Use a
/// trusted transport/source or configure independently trusted checkpoints on
/// the engine. P2P adapters must resolve peer disagreements before returning a
/// trusted result. Requests should support cancellation; the engine bounds their
/// duration and returns errors without automatically retrying.
#[async_trait]
pub trait FilterSource: Send + Sync {
    /// Fetch an inclusive range of at most 2,000 basic filter hashes.
    async fn get_cfheaders(
        &self,
        start_h: u32,
        stop_hash: BlockHash,
    ) -> anyhow::Result<CfHeadersBatch>;
    /// Fetch serialized basic filter bytes (an empty filter is `[0]`).
    async fn get_cfilter(&self, block: BlockHash) -> anyhow::Result<Vec<u8>>;
    /// Fetch a raw consensus-encoded block, including witness data when present.
    ///
    /// Network adapters should choose block-download peers to suit their privacy
    /// model. The engine verifies the block hash and transaction commitments.
    async fn get_block(&self, block: BlockHash) -> anyhow::Result<Vec<u8>>;
}
