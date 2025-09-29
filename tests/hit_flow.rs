use async_trait::async_trait;
use bitcoin::bip158::BlockFilter;
use bitcoin::{
    bip158::Error as BfError,
    block::{Header as BlockHeader, Version as BlockVersion},
    consensus,
    hash_types::TxMerkleNode,
    hashes::Hash,
    pow::CompactTarget,
    Amount, Block, BlockHash, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    WPubkeyHash, Witness,
};
use niebla_158::filter_source::CfHeadersBatch;
use niebla_158::headers::HeaderSource;
use niebla_158::prelude::*;
use std::sync::{Arc, Mutex};

/// ------- Minimal in-memory Store -------
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

/// ------- Wallet hooks: watchlist + hit recorder -------
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

/// ------- Header source that knows about exactly one block at height 1 -------
struct OneHeader {
    bh: BlockHash,
}
#[async_trait]
impl HeaderSource for OneHeader {
    async fn tip_height(&self) -> anyhow::Result<u32> {
        Ok(1)
    }
    async fn hash_at_height(&self, h: u32) -> anyhow::Result<BlockHash> {
        if h == 1 {
            Ok(self.bh)
        } else {
            anyhow::bail!("out of range");
        }
    }
}

/// ------- Filter source that advances cfheaders and returns a matching filter -------
struct OneHitSource {
    block_bytes: Vec<u8>,
    block_hash: BlockHash,
    filter_bytes: Vec<u8>,
}
#[async_trait]
impl FilterSource for OneHitSource {
    async fn get_cfheaders(
        &self,
        start_h: u32,
        _stop: BlockHash,
    ) -> anyhow::Result<CfHeadersBatch> {
        // Advance by one header so cf-tip becomes height 1.
        Ok(CfHeadersBatch {
            start_height: start_h,
            headers: vec![[0u8; 32]],
        })
    }
    async fn get_cfilter(&self, block: BlockHash) -> anyhow::Result<Vec<u8>> {
        if block == self.block_hash {
            Ok(self.filter_bytes.clone())
        } else {
            Ok(Vec::new())
        }
    }
    async fn get_block(&self, block: BlockHash) -> anyhow::Result<Vec<u8>> {
        if block == self.block_hash {
            Ok(self.block_bytes.clone())
        } else {
            anyhow::bail!("unknown block")
        }
    }
}

/// Build a tiny block with one output paying to `watch_script`.
fn make_block_with_output(watch_script: &ScriptBuf) -> Block {
    // coinbase-ish input (not validated by parser)
    let coinbase_in = TxIn {
        previous_output: OutPoint {
            txid: Txid::from_byte_array([0u8; 32]),
            vout: u32::MAX,
        },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::MAX,
        witness: Witness::new(),
    };

    let out = TxOut {
        value: Amount::from_sat(50_000),
        script_pubkey: watch_script.clone(),
    };

    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![coinbase_in],
        output: vec![out],
    };

    let header = BlockHeader {
        version: BlockVersion::from_consensus(2),
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::all_zeros(),
        time: 0,
        bits: CompactTarget::from_consensus(0x207fffff), // easy target (regtest-like)
        nonce: 0,
    };

    Block {
        header,
        txdata: vec![tx],
    }
}

#[tokio::test]
async fn engine_hits_when_filter_contains_watch_script() -> anyhow::Result<()> {
    // Prepare a P2WPKH script: OP_0 <20-byte-hash>
    let wpkh = WPubkeyHash::from_byte_array([7u8; 20]);
    let watch_script = ScriptBuf::new_p2wpkh(&wpkh);

    // Build block and its serialized bytes
    let block = make_block_with_output(&watch_script);
    let block_hash = block.block_hash();
    let block_bytes = consensus::encode::serialize(&block);

    // Build a real BIP158 filter that includes our `watch_script`
    let bf =
        BlockFilter::new_script_filter(&block, |_op: &OutPoint| -> Result<ScriptBuf, BfError> {
            // No prevout scripts in this coinbase-only test — return an empty script.
            Ok(ScriptBuf::new())
        })?;

    let filter_bytes = bf.content.clone();

    let store = MemStore::new();
    let hits: Arc<Mutex<Vec<(u32, BlockHash, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHooks {
        watch: vec![watch_script.clone()],
        hits: hits.clone(),
    };
    let headers = OneHeader { bh: block_hash };
    let source = OneHitSource {
        block_bytes,
        block_hash,
        filter_bytes,
    };

    let engine = Niebla158::new(store, hooks, source, headers);

    // Run: cfheaders will advance to height 1, engine fetches filter, sees the hit, fetches block.
    engine.run_to_tip().await?;

    // Assert exactly one matching block was reported.
    let got = hits.lock().unwrap();
    assert_eq!(got.len(), 1, "expected one matching block");
    assert_eq!(got[0].0, 1); // height
    assert_eq!(got[0].1, block_hash); // block hash
    assert_eq!(got[0].2, 1); // tx count in our fake block

    Ok(())
}
