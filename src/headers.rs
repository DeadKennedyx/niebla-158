//! Access to a validated Bitcoin header chain.
use async_trait::async_trait;
use bitcoin::{block::Header, BlockHash};

/// A height and hash identifying one snapshot of the best chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainTip {
    /// Height of the block, including height zero for genesis.
    pub height: u32,
    /// Hash of that block's header.
    pub hash: BlockHash,
}

/// Provider of a validated best-work header chain.
///
/// Implementations must validate consensus header rules, difficulty and fork
/// choice themselves, or delegate to a trusted full node. The engine checks
/// identity and linkage, but does not implement header-chain consensus.
#[async_trait]
pub trait HeaderSource: Send + Sync {
    /// Return the height and hash from the same best-chain snapshot.
    async fn tip(&self) -> anyhow::Result<ChainTip>;
    /// Return the current best-chain header at `height`, including genesis.
    async fn header_at_height(&self, height: u32) -> anyhow::Result<Header>;
}
