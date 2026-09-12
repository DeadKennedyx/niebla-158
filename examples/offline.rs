//! Runnable complete engine flow using a known local genesis block.
use anyhow::{ensure, Result};
use async_trait::async_trait;
use bitcoin::{
    bip158::{BlockFilter, FilterHash, FilterHeader},
    block::Header,
    blockdata::constants::genesis_block,
    consensus,
    hashes::Hash,
    Block, BlockHash, Network, OutPoint, ScriptBuf, Transaction,
};
use niebla_158::prelude::*;

#[derive(Clone)]
struct LocalSource {
    block: Block,
    filter: Vec<u8>,
}

#[async_trait]
impl HeaderSource for LocalSource {
    async fn tip(&self) -> Result<ChainTip> {
        Ok(ChainTip {
            height: 0,
            hash: self.block.block_hash(),
        })
    }
    async fn header_at_height(&self, height: u32) -> Result<Header> {
        ensure!(height == 0, "this example only contains genesis");
        Ok(self.block.header)
    }
}

#[async_trait]
impl FilterSource for LocalSource {
    async fn get_cfheaders(&self, start: u32, stop: BlockHash) -> Result<CfHeadersBatch> {
        ensure!(
            start == 0 && stop == self.block.block_hash(),
            "invalid example range"
        );
        Ok(CfHeadersBatch {
            filter_type: 0,
            start_height: 0,
            stop_hash: stop,
            previous_filter_header: FilterHeader::all_zeros(),
            filter_hashes: vec![FilterHash::hash(&self.filter)],
        })
    }
    async fn get_cfilter(&self, hash: BlockHash) -> Result<Vec<u8>> {
        ensure!(hash == self.block.block_hash(), "unknown example block");
        Ok(self.filter.clone())
    }
    async fn get_block(&self, hash: BlockHash) -> Result<Vec<u8>> {
        ensure!(hash == self.block.block_hash(), "unknown example block");
        Ok(consensus::serialize(&self.block))
    }
}

struct Wallet {
    script: ScriptBuf,
}
#[async_trait]
impl WalletHooks for Wallet {
    async fn watchlist(&self) -> Result<Vec<ScriptBuf>> {
        Ok(vec![self.script.clone()])
    }
    async fn on_block_match(
        &self,
        height: u32,
        block: BlockHash,
        txs: Vec<Transaction>,
    ) -> Result<()> {
        // Logging only: this is filter matching, not crediting spendable funds.
        println!(
            "Matched height {height}, block {block}, {} transactions",
            txs.len()
        );
        Ok(())
    }
    async fn on_rollback(&self, retained: Option<u32>) -> Result<()> {
        println!("Replay above {retained:?}");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let block = genesis_block(Network::Regtest);
    let filter = BlockFilter::new_script_filter(&block, |_: &OutPoint| {
        Ok::<_, bitcoin::bip158::Error>(ScriptBuf::new())
    })?
    .content;
    let wallet = Wallet {
        script: block.txdata[0].output[0].script_pubkey.clone(),
    };
    // Genesis is unspendable; its script is only a deterministic match fixture.
    let checkpoint = FilterHash::hash(&filter).filter_header(&FilterHeader::all_zeros());
    let source = LocalSource { block, filter };
    let engine = Niebla158::new(
        SqliteStore::new_in_memory()?,
        wallet,
        source.clone(),
        source,
    )
    .with_checkpoints(vec![(0, checkpoint)]);
    println!("{:?}", engine.run_to_tip().await?);
    Ok(())
}
