//! Wallet callbacks, including rollback and replay.
use async_trait::async_trait;
use bitcoin::{BlockHash, ScriptBuf, Transaction};

/// Wallet integration. All callbacks must be durable and idempotent.
///
/// A callback can be replayed after a crash or a failed progress write. The
/// engine invokes callbacks before persisting progress or rollback, so a failed
/// callback never causes it to skip the corresponding work on the next run.
#[async_trait]
pub trait WalletHooks: Send + Sync {
    /// Scripts to watch, refreshed before each block.
    ///
    /// Include the scriptPubKeys of owned outputs to detect spends. Outpoint
    /// bytes themselves are not basic-filter elements. Returning an empty list
    /// pauses scanning without advancing the cursor. Use `rescan_from` for
    /// imported scripts that may have appeared in already-scanned history.
    async fn watchlist(&self) -> anyhow::Result<Vec<ScriptBuf>>;
    /// Receive all transactions in a verified matching block.
    ///
    /// Determine wallet relevance, including spends and filter false positives,
    /// here. Derive the required address lookahead before returning. Replaying
    /// this callback must not duplicate credits or other wallet effects.
    async fn on_block_match(
        &self,
        height: u32,
        block: BlockHash,
        txs: Vec<Transaction>,
    ) -> anyhow::Result<()>;
    /// Remove wallet block effects above `retained_height` before replay.
    ///
    /// `None` removes all scanned block effects, including genesis. This is
    /// called for reorganizations and explicit rescans, and may repeat after a
    /// crash. It must also remove effects from callbacks whose progress write
    /// failed. Do not discard watchlist derivations needed for discovery.
    async fn on_rollback(&self, retained_height: Option<u32>) -> anyhow::Result<()>;
}
