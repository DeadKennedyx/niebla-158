use async_trait::async_trait;
use bitcoin::BlockHash;

/// Source of block header information (height ↔ hash).
#[async_trait]
pub trait HeaderSource: Send + Sync {
    /// Current best height.
    async fn tip_height(&self) -> anyhow::Result<u32>;

    /// Block hash at an exact height.
    async fn hash_at_height(&self, height: u32) -> anyhow::Result<BlockHash>;
}
