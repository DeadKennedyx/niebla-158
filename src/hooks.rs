//! Wallet glue: provide watchlist items and receive notifications on matches.
use async_trait::async_trait;
use bitcoin::{BlockHash, ScriptBuf, Transaction};

#[async_trait]
/// Return scripts/addresses/outpoints to watch for in BIP-158 filters.
pub trait WalletHooks: Send + Sync {
    /// Return scripts/addresses/outpoints to watch for in BIP-158 filters.
    async fn watchlist(&self) -> anyhow::Result<Vec<ScriptBuf>>;
    /// Called when a block at `height` with hash `block` matches the watchlist.
    /// `txs` are the decoded transactions from that block.
    async fn on_block_match(
        &self,
        height: u32,
        block: BlockHash,
        txs: Vec<Transaction>,
    ) -> anyhow::Result<()>;
}
