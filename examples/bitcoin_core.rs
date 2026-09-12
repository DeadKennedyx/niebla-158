//! Run with `cargo run --features rpc --example bitcoin_core`.
//! This demo keeps wallet matches and scan state in memory for one run.
use anyhow::{Context, Result};
use async_trait::async_trait;
use bitcoin::{BlockHash, ScriptBuf, Transaction};
use niebla_158::{
    prelude::*,
    rpc::{BitcoinCoreRpc, RpcAuth},
};
use std::{collections::BTreeMap, env, path::PathBuf, sync::Mutex};

struct Wallet {
    script: ScriptBuf,
    blocks: Mutex<BTreeMap<u32, (BlockHash, Vec<Transaction>)>>,
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
        // Store all transactions; a real wallet must determine relevant receives
        // and spends. Keying by height makes callback replay idempotent.
        self.blocks.lock().unwrap().insert(height, (block, txs));
        println!("Matching block at {height}: {block}");
        Ok(())
    }
    async fn on_rollback(&self, retained: Option<u32>) -> Result<()> {
        self.blocks
            .lock()
            .unwrap()
            .retain(|height, _| Some(*height) <= retained);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let endpoint =
        env::var("BITCOIN_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8332".to_owned());
    let cookie = PathBuf::from(env::var("BITCOIN_COOKIE_FILE").context("set BITCOIN_COOKIE_FILE")?);
    let script = ScriptBuf::from_bytes(hex::decode(
        env::var("WATCH_SCRIPT_HEX").context("set WATCH_SCRIPT_HEX")?,
    )?);
    let source = BitcoinCoreRpc::new(&endpoint, RpcAuth::CookieFile(cookie))?;
    let store = SqliteStore::new_in_memory()?;
    if let Ok(height) = env::var("BIRTH_HEIGHT") {
        store.set_birth_height(Some(height.parse()?)).await?;
    }
    let wallet = Wallet {
        script,
        blocks: Mutex::new(BTreeMap::new()),
    };
    // This choice explicitly trusts the configured full node and transport.
    let engine = Niebla158::new(store, wallet, source.clone(), source).with_trusted_filter_source();
    println!("{:?}", engine.run_to_tip().await?);
    Ok(())
}
