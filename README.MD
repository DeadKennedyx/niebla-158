# niebla-158
this library is **Still a work in progress**

**BIP-158 compact filter** client engine for privacy wallets.

It verifies **cfheaders** (rolling compact-filter headers) against checkpoints, scans **block filters** for your wallet’s scripts/addresses, and fetches matching blocks to hand back relevant transactions — without revealing your whole address list to a third-party server.

## What this crate gives you

- `Niebla158` — the orchestrator (verify → scan → fetch → notify).
- `FilterSource` — trait you implement to fetch:
  - cfheaders batches,
  - per-block compact filters,
  - raw blocks when a filter hits.
- `WalletHooks` — trait your wallet implements to:
  - provide a **watchlist** (scripts/addresses outpoints),
  - receive **on_block_match(height, hash, txs)** callbacks.
- `Store` — tiny persistence layer for:
  - latest verified cfheaders tip,
  - last scanned height,
  - optional birth height.
- `SqliteStore` — bundled, embedded SQLite implementation (no system SQLite needed).

## How you integrate it

You implement three small traits that the engine depends on:

FilterSource — provide cfheaders batches, per-block cfilters, and raw blocks (bytes).

WalletHooks — return your watchlist (scripts) and handle on_block_match callbacks.

Store — persist a couple of integers (verified cfheaders tip and last scanned height).
A bundled SQLite store is available behind the store-sqlite feature.

Because the engine consumesw bytes at the boundary, its agnostic to which network client you use.

Using Nakamoto

You can use Nakamoto as your FilterSource by adding a tiny adapter that converts its responses into the byte shapes this crate expects (cfheaders as [u8; 32] hashes, cfilters as raw bytes, blocks as raw bytes).

## Status / Future plans

Today: the crate ships the engine + traits and SQLite store.

Planned: provide a first-party FilterSource implementation backed by Nakamoto and expose convenient constructors.

## Example use:

```rust
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use bitcoin::{consensus, BlockHash, ScriptBuf, Transaction};
use niebla_158::prelude::*;
use niebla_158::store::sqlite_store::SqliteStore;

// --- 1) Nakamoto adapter ----------------------------------------------------
// Replace these `nakamoto::*` paths with the real ones from the crate you use.
mod naka {
    pub mod client {
        use super::types::*;
        #[derive(Clone)]
        pub struct Handle;
        impl Handle {
            pub async fn connect_testnet() -> anyhow::Result<Self> { unimplemented!() }
            pub async fn tip_height(&self) -> anyhow::Result<u32> { unimplemented!() }
            pub async fn header_hash(&self, _h: u32) -> anyhow::Result<BlockHash> { unimplemented!() }
            pub async fn get_cfheaders(&self, _start: u32, _stop: BlockHash) -> anyhow::Result<Vec<[u8; 32]>> { unimplemented!() }
            pub async fn get_cfilter_bytes(&self, _bh: BlockHash) -> anyhow::Result<Vec<u8>> { unimplemented!() }
            pub async fn get_block_bytes(&self, _bh: BlockHash) -> anyhow::Result<Vec<u8>> { unimplemented!() }
        }
        pub mod types { pub use bitcoin::BlockHash; }
    }
}
use naka::client::Handle as NakaHandle;

/// Single adapter that implements BOTH `FilterSource` and `HeaderSource`.
#[derive(Clone)]
struct NakaSource {
    node: Arc<NakaHandle>,
}

impl NakaSource {
    async fn connect() -> Result<Self> {
        let node = NakaHandle::connect_testnet().await?;
        Ok(Self { node: Arc::new(node) })
    }
}

#[async_trait]
impl FilterSource for NakaSource {
    async fn get_cfheaders(&self, start_h: u32, stop: BlockHash) -> Result<niebla_158::filter_source::CfHeadersBatch> {
        let headers = self.node.get_cfheaders(start_h, stop).await?;
        Ok(niebla_158::filter_source::CfHeadersBatch { start_height: start_h, headers })
    }

    async fn get_cfilter(&self, block: BlockHash) -> Result<Vec<u8>> {
        // Raw BIP158 filter bytes for this block
        self.node.get_cfilter_bytes(block).await
    }

    async fn get_block(&self, block: BlockHash) -> Result<Vec<u8>> {
        // Raw block bytes (can also fetch structured block & `serialize` here)
        self.node.get_block_bytes(block).await
    }
}

#[async_trait]
impl niebla_158::headers::HeaderSource for NakaSource {
    async fn tip_height(&self) -> Result<u32> {
        self.node.tip_height().await
    }

    async fn hash_at_height(&self, h: u32) -> Result<BlockHash> {
        self.node.header_hash(h).await
    }
}

// --- 2) Your wallet hooks ---------------------------------------------------
struct MyWalletHooks {
    watch: Vec<ScriptBuf>,
}

#[async_trait]
impl WalletHooks for MyWalletHooks {
    async fn watchlist(&self) -> Result<Vec<ScriptBuf>> {
        // Provide bech32/legacy scripts you want to track.
        // In a real app, derive from your xpubs or address book.
        Ok(self.watch.clone())
    }

    async fn on_block_match(&self, height: u32, block: BlockHash, txs: Vec<Transaction>) -> Result<()> {
        println!("Hit at {} ({}) with {} txs", height, block, txs.len());
        Ok(())
    }
}

// --- 3) Wire it up ---
#[tokio::main]
async fn main() -> Result<()> {
    // (a) Store: use the bundled SQLite store
    let store = SqliteStore::new("niebla158.db")?;

    // (b) Source & headers: both backed by Nakamoto
    let source = NakaSource::connect().await?;
    let headers = source.clone();

    // (c) Hooks: your wallet watchlist (example: one P2WPKH script)
    use bitcoin::{Address, Network};
    let addr = Address::from_str("tb1qexample...").unwrap()
        .require_network(Network::Testnet).unwrap();
    let watch_script = addr.script_pubkey();
    let hooks = MyWalletHooks { watch: vec![watch_script] };

    // (d) Engine
    let engine = Niebla158::new(store, hooks, source, headers);
    // Optional: add checkpoints once you have a list
    // let engine = engine.with_checkpoints(your_checkpoints_vec);

    // (e) Go! The engine will:
    //     - verify cfheaders
    //     - stream cfilters for new heights, match against your watchlist
    //     - fetch & decode blocks on a hit and call `on_block_match`
    engine.run_to_tip().await?;
    Ok(())
}
```
