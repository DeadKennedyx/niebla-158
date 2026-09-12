//! Authenticated filter synchronization, wallet scanning, and chain rollback.
use crate::{
    cfheaders::CfHeaderChain,
    filter_source::FilterSource,
    headers::{ChainTip, HeaderSource},
    hooks::WalletHooks,
    matcher::filter_matches_any,
    store::{FilterRecord, Store},
};
use anyhow::{ensure, Context, Result};
use bitcoin::{
    bip158::FilterHash, bip158::FilterHeader, consensus, hashes::Hash, Block, BlockHash,
};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    time::Duration,
};
use tokio::sync::Mutex;

const CFHEADERS_BATCH: u32 = 2_000;

/// Progress from one finite synchronization attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncProgress {
    /// Best header-chain snapshot observed for this run.
    pub chain_tip: ChainTip,
    /// Last commitment authenticated by this run's trust policy.
    /// In checkpoint mode this may precede `chain_tip`.
    pub verified_tip: ChainTip,
    /// Last scanned height; `None` means no block has been scanned yet.
    pub last_scanned: Option<u32>,
}

/// Verify filters and matching blocks before notifying a wallet.
///
/// Configure independently trusted checkpoints or explicitly opt into trusting
/// the filter source. Use one engine per wallet/store. Concurrent calls on the
/// same engine are serialized. Errors retain resumable progress; retrying is an
/// explicit caller decision, and wallet callbacks must tolerate replay.
pub struct Niebla158<S, W, F, H> {
    store: S,
    hooks: W,
    source: F,
    headers: H,
    checkpoints: Vec<(u32, FilterHeader)>,
    trusted_source: bool,
    request_timeout: Duration,
    operation: Mutex<()>,
}

impl<S: Store, W: WalletHooks, F: FilterSource, H: HeaderSource> Niebla158<S, W, F, H> {
    /// Create an engine. Scanning requires a trust policy to be configured.
    pub fn new(store: S, hooks: W, source: F, headers: H) -> Self {
        Self {
            store,
            hooks,
            source,
            headers,
            checkpoints: vec![],
            trusted_source: false,
            request_timeout: Duration::from_secs(60),
            operation: Mutex::new(()),
        }
    }

    /// Authenticate history using independently trusted rolling filter headers.
    ///
    /// Without `with_trusted_filter_source`, only history through the latest
    /// checkpoint reached by the header chain is scanned. Future checkpoints
    /// cannot authenticate an unfinished prefix. Duplicate heights are rejected.
    pub fn with_checkpoints(mut self, checkpoints: Vec<(u32, FilterHeader)>) -> Self {
        self.checkpoints = checkpoints;
        self
    }

    /// Explicitly trust the source's filter hashes, for example your own Bitcoin
    /// Core node over local RPC. Configured checkpoints are still enforced.
    ///
    /// Do not use this to treat an arbitrary single P2P peer as authenticated.
    pub fn with_trusted_filter_source(mut self) -> Self {
        self.trusted_source = true;
        self
    }

    /// Bound each header/filter/block request (default: 60 seconds).
    /// A timeout aborts this attempt; call `run_to_tip` again to resume.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Synchronize to the best snapshot permitted by the trust policy, then scan
    /// from the last cursor or birth height. An empty watchlist pauses scanning.
    pub async fn run_to_tip(&self) -> Result<SyncProgress> {
        let _operation = self.operation.lock().await;
        self.run(None).await
    }

    /// Replay history from `start`, including already-scanned blocks.
    ///
    /// Notifies the wallet to roll back first. This explicit start overrides the
    /// birth height for this run and must not exceed the verified tip. The
    /// wallet must retain/derive the watchlist needed for replay.
    pub async fn rescan_from(&self, start: u32) -> Result<SyncProgress> {
        let _operation = self.operation.lock().await;
        self.run(Some(start)).await
    }

    async fn request<T>(&self, request: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(self.request_timeout, request)
            .await
            .context("network request timed out")?
    }

    async fn assert_snapshot(&self, snapshot: ChainTip) -> Result<()> {
        let header = self
            .request(self.headers.header_at_height(snapshot.height))
            .await?;
        ensure!(
            header.block_hash() == snapshot.hash,
            "header chain changed during synchronization; retry to reconcile"
        );
        Ok(())
    }

    async fn record(&self, height: u32) -> Result<FilterRecord> {
        self.store
            .load_filter(height)
            .await?
            .with_context(|| format!("missing stored filter commitment at {height}"))
    }

    async fn reconcile(&self, snapshot: ChainTip) -> Result<()> {
        let genesis = self.request(self.headers.header_at_height(0)).await?;
        ensure!(
            genesis.prev_blockhash == BlockHash::all_zeros(),
            "invalid genesis predecessor"
        );
        let Some(old_tip) = self.store.load_cf_tip().await? else {
            ensure!(
                self.store.get_last_scanned().await?.is_none(),
                "scan cursor without filter commitments"
            );
            return Ok(());
        };
        ensure!(
            self.record(0).await?.block_hash == genesis.block_hash(),
            "store belongs to a different Bitcoin network"
        );
        let mut height = old_tip.height.min(snapshot.height);
        loop {
            let stored = self.record(height).await?;
            let current = self.request(self.headers.header_at_height(height)).await?;
            if stored.block_hash == current.block_hash() {
                self.assert_snapshot(snapshot).await?;
                if height != old_tip.height {
                    // Callback before atomic rewind: on failure/crash it is safe
                    // to repeat, including effects of uncommitted match callbacks.
                    self.hooks
                        .on_rollback(Some(height))
                        .await
                        .context("wallet reorg rollback")?;
                    self.store.rewind(Some(height)).await?;
                }
                return Ok(());
            }
            height = height
                .checked_sub(1)
                .context("no common genesis with stored chain")?;
        }
    }

    async fn verify_stored_checkpoints(
        &self,
        checkpoints: &BTreeMap<u32, FilterHeader>,
    ) -> Result<()> {
        if let Some(tip) = self.store.load_cf_tip().await? {
            for (&height, expected) in checkpoints.range(..=tip.height) {
                ensure!(
                    self.record(height).await?.filter_header == *expected,
                    "stored checkpoint mismatch at {height}"
                );
            }
        }
        Ok(())
    }

    async fn run(&self, rescan: Option<u32>) -> Result<SyncProgress> {
        let mut checkpoints = BTreeMap::new();
        for &(height, header) in &self.checkpoints {
            ensure!(
                checkpoints.insert(height, header).is_none(),
                "duplicate checkpoint height {height}"
            );
        }
        ensure!(
            self.trusted_source || !checkpoints.is_empty(),
            "configure trusted checkpoints or explicitly trust the filter source"
        );
        let snapshot = self
            .request(self.headers.tip())
            .await
            .context("get header-chain tip")?;
        self.assert_snapshot(snapshot).await?;
        let end = if self.trusted_source {
            snapshot.height
        } else {
            *checkpoints
                .range(..=snapshot.height)
                .next_back()
                .context("header chain has not reached a trusted checkpoint")?
                .0
        };
        if let Some(start) = rescan {
            ensure!(start <= end, "rescan start exceeds verified tip");
        }
        self.reconcile(snapshot).await?;
        self.verify_stored_checkpoints(&checkpoints).await?;

        let mut tip = self.store.load_cf_tip().await?;
        let mut chain = CfHeaderChain::new(tip.map(|r| (r.height, r.filter_header)));
        loop {
            let next = match tip {
                Some(record) if record.height >= end => break,
                Some(record) => record.height + 1,
                None => 0,
            };
            let stop = next.saturating_add(CFHEADERS_BATCH - 1).min(end);
            let mut block_hashes = Vec::with_capacity((stop - next + 1) as usize);
            let mut previous_block = tip
                .map(|r| r.block_hash)
                .unwrap_or_else(BlockHash::all_zeros);
            for height in next..=stop {
                let header = self.request(self.headers.header_at_height(height)).await?;
                ensure!(
                    header.prev_blockhash == previous_block,
                    "block header linkage mismatch at {height}"
                );
                previous_block = header.block_hash();
                block_hashes.push(previous_block);
            }
            let stop_hash = previous_block;
            let batch = self
                .request(self.source.get_cfheaders(next, stop_hash))
                .await
                .with_context(|| format!("get cfheaders {next}..={stop}"))?;
            ensure!(batch.filter_type == 0, "unsupported filter type");
            ensure!(
                batch.start_height == next && batch.stop_hash == stop_hash,
                "cfheaders response range mismatch"
            );
            ensure!(
                batch.filter_hashes.len() == block_hashes.len(),
                "cfheaders response length mismatch: expected {}, got {}",
                block_hashes.len(),
                batch.filter_hashes.len()
            );
            let rolling = chain.apply_batch(
                next,
                batch.previous_filter_header,
                &batch.filter_hashes,
                &checkpoints,
            )?;
            let records: Vec<_> = block_hashes
                .into_iter()
                .zip(batch.filter_hashes)
                .zip(rolling)
                .enumerate()
                .map(
                    |(i, ((block_hash, filter_hash), filter_header))| FilterRecord {
                        height: next + i as u32,
                        block_hash,
                        filter_hash,
                        filter_header,
                    },
                )
                .collect();
            self.assert_snapshot(snapshot).await?;
            self.store.save_filters(&records).await?;
            tip = records.last().copied();
        }
        // No filter is scanned until the entire requested prefix is anchored.
        self.verify_stored_checkpoints(&checkpoints).await?;
        self.assert_snapshot(snapshot).await?;
        let mut scanned = self.store.get_last_scanned().await?;
        ensure!(
            scanned <= tip.map(|r| r.height),
            "scan cursor exceeds stored commitments"
        );
        ensure!(
            rescan.is_some() || scanned <= Some(end),
            "scan cursor exceeds the configured trust boundary; use rescan_from to replay"
        );
        if let Some(start) = rescan {
            let retained = scanned.min(start.checked_sub(1));
            self.hooks
                .on_rollback(retained)
                .await
                .context("wallet rescan rollback")?;
            self.store.set_last_scanned(retained).await?;
            scanned = retained;
        }
        let floor = match rescan {
            Some(start) => start,
            None => self.store.get_birth_height().await?.unwrap_or(0),
        };
        let next = scanned
            .map(|h| u64::from(h) + 1)
            .unwrap_or(0)
            .max(u64::from(floor));
        for height in next..=u64::from(end) {
            let height = height as u32;
            let watch = self.hooks.watchlist().await?;
            if watch.is_empty() {
                break;
            }
            let record = self.record(height).await?;
            let header = self.request(self.headers.header_at_height(height)).await?;
            ensure!(
                header.block_hash() == record.block_hash,
                "scanned chain changed at {height}; retry to reconcile"
            );
            let raw_filter = self
                .request(self.source.get_cfilter(record.block_hash))
                .await
                .with_context(|| format!("get filter at {height}"))?;
            ensure!(
                FilterHash::hash(&raw_filter) == record.filter_hash,
                "filter commitment mismatch at {height}"
            );
            let hit = filter_matches_any(record.block_hash, &raw_filter, &watch)
                .with_context(|| format!("decode filter at {height}"))?;
            if hit {
                let raw = self
                    .request(self.source.get_block(record.block_hash))
                    .await
                    .with_context(|| format!("get matching block at {height}"))?;
                ensure!(
                    raw.len() <= 4_000_000,
                    "block exceeds maximum serialized size"
                );
                let block: Block = consensus::deserialize(&raw).context("decode matching block")?;
                ensure!(
                    block.block_hash() == record.block_hash,
                    "downloaded block hash mismatch at {height}"
                );
                ensure!(
                    block.check_merkle_root(),
                    "block Merkle root mismatch at {height}"
                );
                // Duplicating the final transaction of an odd-sized Merkle
                // layer can preserve its root. Reject that ambiguous encoding.
                let mut txids = HashSet::with_capacity(block.txdata.len());
                ensure!(
                    block
                        .txdata
                        .iter()
                        .all(|tx| txids.insert(tx.compute_txid())),
                    "duplicate transaction in downloaded block at {height}"
                );
                ensure!(
                    block.weight().to_wu() <= 4_000_000,
                    "block exceeds maximum weight"
                );
                ensure!(
                    block.check_witness_commitment(),
                    "block witness commitment mismatch at {height}"
                );
                self.assert_snapshot(snapshot).await?;
                self.hooks
                    .on_block_match(height, record.block_hash, block.txdata)
                    .await
                    .with_context(|| format!("wallet block callback at {height}"))?;
            }
            self.assert_snapshot(snapshot).await?;
            self.store.set_last_scanned(Some(height)).await?;
        }
        self.assert_snapshot(snapshot).await?;
        let verified = self.record(end).await?;
        Ok(SyncProgress {
            chain_tip: snapshot,
            verified_tip: ChainTip {
                height: end,
                hash: verified.block_hash,
            },
            last_scanned: self.store.get_last_scanned().await?,
        })
    }
}
