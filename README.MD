# niebla-158

**BIP-158 compact filter** client engine for wallets.

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

## Install

```toml
[dependencies]
niebla-158 = { version = "0.1", features = [] }  # sqlite is always on in this crate
anyhow = "1"
async-trait = "0.1"
bitcoin = "0.32"

