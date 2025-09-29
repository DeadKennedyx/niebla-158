# niebla-158

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

How you integrate it

You implement three small traits that the engine depends on:

FilterSource — provide cfheaders batches, per-block cfilters, and raw blocks (bytes).

WalletHooks — return your watchlist (scripts) and handle on_block_match callbacks.

Store — persist a couple of integers (verified cfheaders tip and last scanned height).
A bundled SQLite store is available behind the store-sqlite feature.

Because the engine consumesw bytes at the boundary, its agnostic to which network client you use.

Using Nakamoto

You can use Nakamoto as your FilterSource by adding a tiny adapter that converts its responses into the byte shapes this crate expects (cfheaders as [u8; 32] hashes, cfilters as raw bytes, blocks as raw bytes).

## Install

```toml
[dependencies]
niebla-158 = { version = "0.1", features = [] }  # sqlite is always on in this crate
anyhow = "1"
async-trait = "0.1"
bitcoin = "0.32"

