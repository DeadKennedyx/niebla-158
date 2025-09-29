use async_trait::async_trait;
use bitcoin::hashes::{sha256d, Hash as _};
use bitcoin::{self, BlockHash, ScriptBuf, Transaction};
use niebla_158::filter_source::CfHeadersBatch;
use niebla_158::headers::HeaderSource;
use niebla_158::prelude::*; // Niebla158, Store, WalletHooks, FilterSource
use std::sync::{Arc, Mutex};

/// Minimal in-memory Store for tests (keeps engine generic & fast).
struct MemStore {
    cf_tip: Mutex<Option<(u32, BlockHash)>>,
    last_scanned: Mutex<u32>,
    birth: Mutex<Option<u32>>,
}
impl MemStore {
    fn new() -> Self {
        Self {
            cf_tip: Mutex::new(None),
            last_scanned: Mutex::new(0),
            birth: Mutex::new(None),
        }
    }
}

#[async_trait]
impl Store for MemStore {
    async fn load_cf_tip(&self) -> anyhow::Result<Option<(u32, BlockHash)>> {
        Ok(*self.cf_tip.lock().unwrap())
    }
    async fn save_cf_tip(&self, height: u32, cfheader: BlockHash) -> anyhow::Result<()> {
        *self.cf_tip.lock().unwrap() = Some((height, cfheader));
        Ok(())
    }
    async fn get_last_scanned(&self) -> anyhow::Result<u32> {
        Ok(*self.last_scanned.lock().unwrap())
    }
    async fn set_last_scanned(&self, height: u32) -> anyhow::Result<()> {
        *self.last_scanned.lock().unwrap() = height;
        Ok(())
    }
    async fn get_birth_height(&self) -> anyhow::Result<Option<u32>> {
        Ok(*self.birth.lock().unwrap())
    }
    async fn set_birth_height(&self, h: u32) -> anyhow::Result<()> {
        *self.birth.lock().unwrap() = Some(h);
        Ok(())
    }
}

/// Wallet hooks: a tiny watchlist and a hit recorder.
struct TestHooks {
    watch: Vec<ScriptBuf>,
    hits: Arc<Mutex<Vec<(u32, BlockHash, usize)>>>, // (height, block, tx_count)
}
#[async_trait]
impl WalletHooks for TestHooks {
    async fn watchlist(&self) -> anyhow::Result<Vec<ScriptBuf>> {
        Ok(self.watch.clone())
    }
    async fn on_block_match(
        &self,
        height: u32,
        block: BlockHash,
        txs: Vec<Transaction>,
    ) -> anyhow::Result<()> {
        self.hits.lock().unwrap().push((height, block, txs.len()));
        Ok(())
    }
}

/// Header source stub: no headers/tip.
struct NoHeaders;

#[async_trait]
impl HeaderSource for NoHeaders {
    async fn tip_height(&self) -> anyhow::Result<u32> {
        Ok(0)
    }

    async fn hash_at_height(&self, _h: u32) -> anyhow::Result<BlockHash> {
        // zero hash is fine for a stub
        Ok(BlockHash::from_raw_hash(sha256d::Hash::all_zeros()))
    }
}
/// Filter source with no matches (empty filters) — keeps test deterministic.
struct NoHitSource;
#[async_trait]
impl FilterSource for NoHitSource {
    async fn get_cfheaders(
        &self,
        start_h: u32,
        _stop: BlockHash,
    ) -> anyhow::Result<CfHeadersBatch> {
        Ok(CfHeadersBatch {
            start_height: start_h,
            headers: vec![],
        })
    }
    async fn get_cfilter(&self, _block: BlockHash) -> anyhow::Result<Vec<u8>> {
        Ok(Vec::new()) // empty filter → no hits
    }
    async fn get_block(&self, _block: BlockHash) -> anyhow::Result<Vec<u8>> {
        Ok(Vec::new()) // never called because no hits
    }
}

#[tokio::test]
async fn engine_compiles_and_runs_with_no_hits() -> anyhow::Result<()> {
    let store = MemStore::new();

    // One fake script in the watchlist
    let script = bitcoin::ScriptBuf::new(); // empty script (fine for smoke)
    let hits = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHooks {
        watch: vec![script],
        hits: hits.clone(),
    };

    // No matches source + no headers
    let source = NoHitSource;
    let headers = NoHeaders;

    let engine = Niebla158::new(store, hooks, source, headers);

    // New API: no iterator argument
    engine.run_to_tip().await?;

    // No blocks were matched/fetched
    assert!(hits.lock().unwrap().is_empty());

    Ok(())
}
