//! Orchestrator for BIP-158 light client flow:
//! 1) verify cfheaders with optional checkpoints,
//! 2) scan per-block filters against a wallet watchlist,
//! 3) fetch matching blocks and deliver transactions.
use crate::{
    cfheaders::CfHeaderChain, filter_source::FilterSource, headers::HeaderSource,
    hooks::WalletHooks, matcher::filter_matches_any, store::Store,
};
use anyhow::Context;
use bitcoin::{consensus, Block, BlockHash};

/// How many cfheaders to advance per request window.
const CFHEADERS_BATCH: u32 = 2_000;

/// Core engine. `S` = store, `W` = wallet hooks, `F` = network filter source, `H` = header iterator/stream.
pub struct Niebla158<S, W, F, H> {
    store: S,
    hooks: W,
    source: F,
    headers: H,
    checkpoints: Vec<(u32, BlockHash)>,
}

impl<S, W, F, H> Niebla158<S, W, F, H>
where
    S: Store + 'static,
    W: WalletHooks + 'static,
    F: FilterSource + 'static,
    H: HeaderSource + 'static,
{
    /// Create a new engine with a store, wallet hooks, filter source, and a headers provider.
    pub fn new(store: S, hooks: W, source: F, headers: H) -> Self {
        Self {
            store,
            hooks,
            source,
            headers,
            checkpoints: vec![],
        }
    }

    /// Provide compact-filter header checkpoints `(height, rolling_cfheader_hash)` for defense-in-depth
    pub fn with_checkpoints(mut self, v: Vec<(u32, BlockHash)>) -> Self {
        self.checkpoints = v;
        self
    }

    /// Verify/advance compact-filter headers to the given tip and then
    /// scan each block's BIP-158 filter against the wallet watchlist.
    /// For every hit, fetch and decode the block and forward its txs to `WalletHooks`.
    ///
    /// # Arguments
    /// * `header_heights` — iterator of `(height, block_hash)` representing the header chain to follow.
    ///
    /// # Errors
    /// Returns an error if cfheader verification fails, the network source fails to
    /// provide data, block decoding fails, or the store cannot persist progress.
    pub async fn run_to_tip(&self) -> anyhow::Result<()> {
        let cf_tip = self.store.load_cf_tip().await?;
        let mut cfchain = CfHeaderChain::new_from_store(cf_tip);

        let chain_tip = self.headers.tip_height().await?;

        let mut next = cfchain.tip_height.saturating_add(1);
        while next <= chain_tip {
            let stop_h = (next + CFHEADERS_BATCH - 1).min(chain_tip);
            let stop_hash = self.headers.hash_at_height(stop_h).await?;

            let batch = self
                .source
                .get_cfheaders(next, stop_hash)
                .await
                .with_context(|| format!("get_cfheaders(start={next}, stop_h={stop_h})"))?;

            cfchain
                .apply_batch(batch.start_height, &batch.headers, &self.checkpoints)
                .with_context(|| format!("apply cfheaders batch @{}", batch.start_height))?;

            self.store
                .save_cf_tip(cfchain.tip_height, cfchain.tip_hash)
                .await?;

            next = cfchain.tip_height.saturating_add(1);
        }

        // 4) Scan filters from last_scanned+1 ..= cfheaders tip
        let last_scanned = self.store.get_last_scanned().await?;
        let end_h = cfchain.tip_height;

        let watch = self.hooks.watchlist().await?;
        if watch.is_empty() {
            // Nothing to match; mark up-to-date and exit.
            self.store.set_last_scanned(end_h).await?;
            return Ok(());
        }

        for h in (last_scanned + 1)..=end_h {
            let block_hash = self.headers.hash_at_height(h).await?;

            // (a) Pull filter and test
            let raw_filter = self
                .source
                .get_cfilter(block_hash)
                .await
                .with_context(|| format!("get_cfilter({block_hash})"))?;

            let hit = filter_matches_any(block_hash, &raw_filter, watch.clone().into_iter())
                .with_context(|| format!("filter match @height {h}"))?;

            // (b) On hit, download block and callback
            if hit {
                let raw_block = self
                    .source
                    .get_block(block_hash)
                    .await
                    .with_context(|| format!("get_block({block_hash})"))?;

                let block: Block =
                    consensus::encode::deserialize(&raw_block).context("block deserialize")?;
                let txs = block.txdata.clone();

                self.hooks
                    .on_block_match(h, block_hash, txs)
                    .await
                    .with_context(|| format!("on_block_match @height {h}"))?;
            }

            // (c) Persist progress every height
            self.store.set_last_scanned(h).await?;
        }

        Ok(())
    }
}
