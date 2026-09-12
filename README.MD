# niebla-158

A Rust BIP-157/158 client engine that verifies compact-filter commitments, scans wallet scripts, verifies matching blocks, and supports rollback and replay. Version 0.2 makes breaking API and storage changes; read [MIGRATION.md](MIGRATION.md) before upgrading from 0.1.

The engine synchronizes filter hashes from genesis, checks response ranges and rolling headers, and stores each commitment with its block hash. It authenticates downloaded filters before matching, then verifies block identity, transaction Merkle roots, and witness commitments before calling the wallet. Chain reorganizations trigger wallet rollback followed by replay.

You provide four interfaces:

- `HeaderSource`: a validated best-work header chain, including genesis and consistent height/hash snapshots.
- `FilterSource`: basic filter hashes, serialized filters, and serialized blocks. `CfHeadersBatch` contains `FilterHash` values and the preceding `FilterHeader`, not a list of rolling headers.
- `WalletHooks`: a script watchlist, an idempotent block callback, and an idempotent rollback callback.
- `Store`: contiguous block/filter commitments and scan progress. The bundled `SqliteStore` supports disk and memory without an additional feature flag.

Choose a trust policy explicitly. `with_checkpoints` uses independently trusted filter headers and scans only through the latest checkpoint reached by the header chain. `with_trusted_filter_source` allows synchronization to the current tip using a source you trust, such as your own full node over authenticated RPC. Additional checkpoints are enforced in both modes. An arbitrary single peer's internally consistent hashes are not authenticated merely by hashing them again. `SyncProgress` reports the network snapshot, authenticated tip, and scan cursor separately.

This integration function compiles as a doctest. Its source and header provider must satisfy the trust contracts above:

```rust,no_run
use anyhow::Result;
use niebla_158::prelude::*;

async fn scan_with_trusted_source<W, F, H>(
    wallet: W,
    source: F,
    headers: H,
) -> Result<SyncProgress>
where
    W: WalletHooks,
    F: FilterSource,
    H: HeaderSource,
{
    let store = SqliteStore::new("wallet-sync.sqlite")?;
    let engine = Niebla158::new(store, wallet, source, headers)
        .with_trusted_filter_source();
    engine.run_to_tip().await
}
```

A complete, runnable example using a local genesis fixture is included:

```sh
cargo run --example offline
```

The optional `rpc` feature provides `rpc::BitcoinCoreRpc`, implementing both network interfaces for a trusted Bitcoin Core node. Configure the node with `blockfilterindex=1` and wait for its index to synchronize. Use loopback HTTP or HTTPS with certificate validation. The adapter rereads cookie credentials on each request, disables redirects and environment proxies, limits response size, and applies request timeouts. It never sends the watchlist. The node still sees matching block requests, so this transport is intended for a node you trust with that information.

```toml
[dependencies]
niebla-158 = { version = "0.2", features = ["rpc"] }
```

Run the RPC example with a cookie path and the hex scriptPubKey to watch:

```sh
export BITCOIN_RPC_URL=http://127.0.0.1:8332
export BITCOIN_COOKIE_FILE=/path/to/bitcoin/.cookie
export WATCH_SCRIPT_HEX=00140707070707070707070707070707070707070707
export BIRTH_HEIGHT=800000
cargo run --features rpc --example bitcoin_core
```

This example keeps wallet matches and scan state in memory for one run. A production wallet should persist both and make callbacks safe to replay. RPC synchronization fetches headers and filters through individual node calls; initial verification still starts at genesis, even with a recent birth height. On high-latency connections, raise the engine's `with_request_timeout` for a batch or implement a more efficient `FilterSource`. The built-in HTTP timeout can be set separately with `BitcoinCoreRpc::with_timeout`. Errors return resumable progress; retry policy belongs to the caller. A first-party Nakamoto/P2P transport and peer-disagreement resolution are not implemented by this RPC adapter.

Scan and callback behavior:

- `set_birth_height(Some(height))` skips earlier filter downloads and matching, while preserving header verification from genesis. `Some(0)` explicitly includes genesis; `None` leaves the birth height unset.
- The watchlist is refreshed before every block. An empty watchlist pauses scanning without marking history complete. Derive address lookahead in the wallet callback before returning.
- After importing scripts that may appear in old history, call `engine.rescan_from(height).await?`. It requests wallet rollback and replays that range. Its explicit start overrides the birth height for that run.
- `on_block_match` receives **all** transactions in a matching block. The wallet identifies relevant receives and spends and handles false positives. Watch previous outputs' scriptPubKeys to detect spends; outpoint bytes are not basic-filter elements.
- `on_rollback(Some(h))` removes block effects above `h`; `None` removes all scanned block effects. Keep the watchlist derivations needed for replay.
- Callbacks run before their corresponding progress writes. Failures and crashes can cause replay, so callbacks must be durable and idempotent, including rollback after an uncommitted match callback.
- Use one engine per wallet/database. Calls on the same engine are serialized. A reorganization detected during a run returns an error; the next run reconciles stored history before scanning. A change after the final snapshot check is reconciled on the next run.

The crate delegates proof-of-work, difficulty validation, and best-chain selection to `HeaderSource`; the RPC adapter delegates them to Bitcoin Core. Custom P2P adapters must provide those guarantees, bound their resource usage, resolve filter-header disagreements if used as a trusted source, and choose block-download peers according to their privacy requirements. Filter headers are not commitments in Bitcoin's proof-of-work headers; see the [BIP-157 trust model](https://github.com/bitcoin/bips/blob/master/bip-0157.mediawiki#client-operation).

Run the checks locally:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --doc --all-features
```

CI runs these checks, the offline example, and an isolated Bitcoin Core regtest integration. Tests include official [BIP-158 vectors](https://github.com/bitcoin/bips/blob/master/bip-0158/testnet-19.json), malformed responses, forged filters and blocks, witness tampering, reorgs, restart/replay, birth heights, and SQLite atomicity. The fixtures' provenance is recorded in `tests/data/README.md`.

To run the external-node integration locally, set `BITCOIND` to a Bitcoin Core executable, then run `cargo test --features rpc --test rpc_regtest -- --ignored`. The test creates its own temporary regtest node, disables network activity, and stops it afterward.

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
