#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use bitcoin::{
    absolute,
    bip158::{BlockFilter, FilterHash, FilterHeader},
    block::{Header, Version},
    blockdata::constants::genesis_block,
    consensus,
    hashes::Hash,
    transaction, Amount, Block, BlockHash, Network, OutPoint, ScriptBuf, Sequence, Transaction,
    TxIn, TxOut, WPubkeyHash, Witness,
};
use niebla_158::prelude::*;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub fn script(tag: u8) -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([tag; 20]))
}

pub fn next_block(previous: &Block, script: ScriptBuf, height: u32) -> Block {
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: bitcoin::script::Builder::new()
                .push_int(i64::from(height))
                .push_int(0)
                .into_script(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: script,
        }],
    };
    let mut block = Block {
        header: Header {
            version: Version::from_consensus(4),
            prev_blockhash: previous.block_hash(),
            merkle_root: bitcoin::TxMerkleNode::all_zeros(),
            time: previous.header.time + 600,
            bits: previous.header.bits,
            nonce: 0,
        },
        txdata: vec![tx],
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    while block.header.validate_pow(block.header.target()).is_err() {
        block.header.nonce += 1;
    }
    block
}

pub fn filter(block: &Block) -> Vec<u8> {
    BlockFilter::new_script_filter(block, |_: &OutPoint| {
        Ok::<_, bitcoin::bip158::Error>(ScriptBuf::new())
    })
    .unwrap()
    .content
}

#[derive(Clone, Copy)]
pub enum BatchFault {
    Empty,
    Short,
    Long,
    Start,
    Stop,
    Type,
    Previous,
}

pub struct ChainData {
    pub blocks: Vec<Block>,
    pub filters: Vec<Vec<u8>>,
    pub served_filters: HashMap<BlockHash, Vec<u8>>,
    pub served_blocks: HashMap<BlockHash, Vec<u8>>,
    pub batch_fault: Option<BatchFault>,
    pub batch_requests: Vec<(u32, BlockHash)>,
    pub filter_requests: Vec<BlockHash>,
    pub block_requests: Vec<BlockHash>,
    pub reorg_on_filter: Option<(u32, ScriptBuf)>,
}

impl ChainData {
    pub fn records(&self) -> Vec<FilterRecord> {
        let mut previous = FilterHeader::all_zeros();
        self.blocks
            .iter()
            .zip(&self.filters)
            .enumerate()
            .map(|(i, (block, bytes))| {
                let hash = FilterHash::hash(bytes);
                previous = hash.filter_header(&previous);
                FilterRecord {
                    height: i as u32,
                    block_hash: block.block_hash(),
                    filter_hash: hash,
                    filter_header: previous,
                }
            })
            .collect()
    }
    pub fn replace_from(&mut self, height: u32, script: ScriptBuf) {
        let end = self.blocks.len();
        self.blocks.truncate(height as usize);
        for h in height as usize..end {
            let block = next_block(self.blocks.last().unwrap(), script.clone(), h as u32);
            self.blocks.push(block);
        }
        self.filters = self.blocks.iter().map(filter).collect();
    }
}

#[derive(Clone)]
pub struct Chain {
    pub data: Arc<Mutex<ChainData>>,
    pub hang_cfheaders: Arc<AtomicBool>,
}

impl Chain {
    pub fn new(scripts: &[ScriptBuf]) -> Self {
        let mut blocks = vec![genesis_block(Network::Regtest)];
        for (i, script) in scripts.iter().enumerate() {
            blocks.push(next_block(
                blocks.last().unwrap(),
                script.clone(),
                i as u32 + 1,
            ));
        }
        Self::from_blocks(blocks)
    }
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let filters = blocks.iter().map(filter).collect();
        Self {
            data: Arc::new(Mutex::new(ChainData {
                blocks,
                filters,
                served_filters: HashMap::new(),
                served_blocks: HashMap::new(),
                batch_fault: None,
                batch_requests: vec![],
                filter_requests: vec![],
                block_requests: vec![],
                reorg_on_filter: None,
            })),
            hang_cfheaders: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn hash(&self, height: u32) -> BlockHash {
        self.data.lock().unwrap().blocks[height as usize].block_hash()
    }
    pub fn records(&self) -> Vec<FilterRecord> {
        self.data.lock().unwrap().records()
    }
    pub fn block(&self, height: u32) -> Block {
        self.data.lock().unwrap().blocks[height as usize].clone()
    }
}

#[async_trait]
impl HeaderSource for Chain {
    async fn tip(&self) -> Result<ChainTip> {
        let data = self.data.lock().unwrap();
        Ok(ChainTip {
            height: data.blocks.len() as u32 - 1,
            hash: data.blocks.last().unwrap().block_hash(),
        })
    }
    async fn header_at_height(&self, height: u32) -> Result<Header> {
        self.data
            .lock()
            .unwrap()
            .blocks
            .get(height as usize)
            .map(|block| block.header)
            .context("height outside test chain")
    }
}

#[async_trait]
impl FilterSource for Chain {
    async fn get_cfheaders(&self, start: u32, stop: BlockHash) -> Result<CfHeadersBatch> {
        if self.hang_cfheaders.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let mut data = self.data.lock().unwrap();
        data.batch_requests.push((start, stop));
        let records = data.records();
        let stop_height = records
            .iter()
            .position(|r| r.block_hash == stop)
            .context("unknown stop")?;
        let previous = start
            .checked_sub(1)
            .map(|h| records[h as usize].filter_header)
            .unwrap_or_else(FilterHeader::all_zeros);
        let mut batch = CfHeadersBatch {
            filter_type: 0,
            start_height: start,
            stop_hash: stop,
            previous_filter_header: previous,
            filter_hashes: records[start as usize..=stop_height]
                .iter()
                .map(|r| r.filter_hash)
                .collect(),
        };
        match data.batch_fault {
            Some(BatchFault::Empty) => batch.filter_hashes.clear(),
            Some(BatchFault::Short) => {
                batch.filter_hashes.pop();
            }
            Some(BatchFault::Long) => batch.filter_hashes.push(FilterHash::all_zeros()),
            Some(BatchFault::Start) => batch.start_height += 1,
            Some(BatchFault::Stop) => batch.stop_hash = BlockHash::all_zeros(),
            Some(BatchFault::Type) => batch.filter_type = 1,
            Some(BatchFault::Previous) => {
                batch.previous_filter_header = FilterHeader::from_byte_array([1; 32])
            }
            None => {}
        }
        Ok(batch)
    }
    async fn get_cfilter(&self, hash: BlockHash) -> Result<Vec<u8>> {
        let mut data = self.data.lock().unwrap();
        data.filter_requests.push(hash);
        let height = data
            .blocks
            .iter()
            .position(|b| b.block_hash() == hash)
            .context("unknown filter")?;
        let bytes = data
            .served_filters
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| data.filters[height].clone());
        if let Some((height, script)) = data.reorg_on_filter.take() {
            data.replace_from(height, script);
        }
        Ok(bytes)
    }
    async fn get_block(&self, hash: BlockHash) -> Result<Vec<u8>> {
        let mut data = self.data.lock().unwrap();
        data.block_requests.push(hash);
        data.served_blocks
            .get(&hash)
            .cloned()
            .or_else(|| {
                data.blocks
                    .iter()
                    .find(|b| b.block_hash() == hash)
                    .map(consensus::serialize)
            })
            .context("unknown block")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Match(u32, BlockHash),
    Rollback(Option<u32>),
}

#[derive(Clone, Default)]
pub struct Wallet {
    pub watch: Arc<Mutex<Vec<ScriptBuf>>>,
    pub active: Arc<Mutex<BTreeMap<u32, BlockHash>>>,
    pub events: Arc<Mutex<Vec<Event>>>,
    pub expand: Arc<Mutex<Option<ScriptBuf>>>,
    pub fail_match: Arc<AtomicBool>,
    pub fail_rollback: Arc<AtomicBool>,
}

impl Wallet {
    pub fn new(watch: Vec<ScriptBuf>) -> Self {
        Self {
            watch: Arc::new(Mutex::new(watch)),
            ..Self::default()
        }
    }
    pub fn heights(&self) -> Vec<u32> {
        self.active.lock().unwrap().keys().copied().collect()
    }
}

#[async_trait]
impl WalletHooks for Wallet {
    async fn watchlist(&self) -> Result<Vec<ScriptBuf>> {
        Ok(self.watch.lock().unwrap().clone())
    }
    async fn on_block_match(
        &self,
        height: u32,
        hash: BlockHash,
        _txs: Vec<Transaction>,
    ) -> Result<()> {
        if self.fail_match.load(Ordering::SeqCst) {
            bail!("injected wallet match failure");
        }
        self.active.lock().unwrap().insert(height, hash);
        self.events.lock().unwrap().push(Event::Match(height, hash));
        if let Some(script) = self.expand.lock().unwrap().take() {
            self.watch.lock().unwrap().push(script);
        }
        Ok(())
    }
    async fn on_rollback(&self, retained: Option<u32>) -> Result<()> {
        if self.fail_rollback.load(Ordering::SeqCst) {
            bail!("injected wallet rollback failure");
        }
        self.active
            .lock()
            .unwrap()
            .retain(|h, _| Some(*h) <= retained);
        self.events.lock().unwrap().push(Event::Rollback(retained));
        Ok(())
    }
}

pub fn engine(
    store: SqliteStore,
    wallet: Wallet,
    chain: Chain,
) -> Niebla158<SqliteStore, Wallet, Chain, Chain> {
    Niebla158::new(store, wallet, chain.clone(), chain).with_trusted_filter_source()
}
