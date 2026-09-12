//! Bitcoin Core JSON-RPC adapter for a trusted, validating full node.
//!
//! Enable `blockfilterindex=1` on the node. Use loopback HTTP or authenticated
//! HTTPS; this adapter sends no watchlist but the node can observe block requests.
//! It bounds request time and response size, disables redirects/proxies, and does
//! not retry automatically. Cookie credentials are reread to support rotation.
use crate::{CfHeadersBatch, ChainTip, FilterSource, HeaderSource};
use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use bitcoin::{
    bip158::{FilterHash, FilterHeader},
    block::Header,
    consensus,
    hashes::Hash,
    BlockHash,
};
use reqwest::{Client, Url};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    net::IpAddr,
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// RPC credentials. Deliberately does not implement `Debug`.
#[derive(Clone)]
pub enum RpcAuth {
    /// Read `username:password` from Bitcoin Core's cookie file on each request.
    CookieFile(PathBuf),
    /// Explicit RPC credentials; prefer a cookie for a local node.
    UserPass {
        /// RPC username.
        username: String,
        /// RPC password.
        password: String,
    },
}

/// Cloneable source implementing both engine transport traits.
///
/// Trust is explicit: construct an engine with `with_trusted_filter_source`
/// only for a node/connection you trust, or supply independent checkpoints.
#[derive(Clone)]
pub struct BitcoinCoreRpc {
    endpoint: Url,
    auth: RpcAuth,
    client: Client,
    next_id: Arc<AtomicU64>,
    timeout: Duration,
}

#[derive(Deserialize)]
struct CoreFilter {
    filter: String,
    header: String,
}

impl BitcoinCoreRpc {
    /// Configure an authenticated endpoint. Plain HTTP is restricted to loopback.
    /// Credentials, query parameters, and fragments are rejected in the URL.
    pub fn new(endpoint: &str, auth: RpcAuth) -> Result<Self> {
        let endpoint = Url::parse(endpoint).context("invalid Bitcoin Core RPC URL")?;
        ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "provide RPC credentials through RpcAuth, without URL queries or fragments"
        );
        let host = endpoint.host_str().context("RPC URL needs a host")?;
        let loopback = host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false);
        ensure!(
            endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback),
            "RPC requires HTTPS or loopback HTTP"
        );
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .context("build RPC HTTP client")?;
        Ok(Self {
            endpoint,
            auth,
            client,
            next_id: Arc::new(AtomicU64::new(1)),
            timeout: Duration::from_secs(30),
        })
    }

    /// Set the total timeout for an individual HTTP request (default: 30 seconds).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn credentials(&self) -> Result<(String, String)> {
        match &self.auth {
            RpcAuth::CookieFile(path) => {
                let cookie = tokio::fs::read_to_string(path)
                    .await
                    .context("read Bitcoin Core RPC cookie")?;
                let (user, password) = cookie
                    .trim_end()
                    .split_once(':')
                    .context("invalid RPC cookie format")?;
                ensure!(
                    !user.is_empty() && !password.is_empty(),
                    "RPC cookie contains empty credentials"
                );
                Ok((user.to_owned(), password.to_owned()))
            }
            RpcAuth::UserPass { username, password } => Ok((username.clone(), password.clone())),
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &'static str, params: Value) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (user, password) = self.credentials().await?;
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .basic_auth(user, Some(password))
            .timeout(self.timeout)
            .json(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .send()
            .await
            .map_err(|error| error.without_url())
            .with_context(|| format!("RPC {method} request"))?;
        let status = response.status();
        if let Some(length) = response.content_length() {
            ensure!(
                length <= MAX_RESPONSE_BYTES as u64,
                "RPC response exceeds size limit"
            );
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url())
            .context("read RPC response")?
        {
            ensure!(
                body.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
                "RPC response exceeds size limit"
            );
            body.extend_from_slice(&chunk);
        }
        let envelope: Value = serde_json::from_slice(&body).context("invalid RPC JSON response")?;
        ensure!(
            envelope.get("id").and_then(Value::as_u64) == Some(id),
            "RPC response ID mismatch"
        );
        if let Some(error) = envelope.get("error").filter(|error| !error.is_null()) {
            anyhow::bail!(
                "Bitcoin Core RPC {method} failed with code {}",
                error.get("code").unwrap_or(&Value::Null)
            );
        }
        ensure!(status.is_success(), "RPC HTTP status {status}");
        serde_json::from_value(
            envelope
                .get("result")
                .context("missing RPC result")?
                .clone(),
        )
        .with_context(|| format!("invalid RPC {method} result"))
    }

    async fn block_hash_at(&self, height: u32) -> Result<BlockHash> {
        let hash: String = self.call("getblockhash", json!([height])).await?;
        BlockHash::from_str(&hash).context("invalid RPC block hash")
    }

    async fn basic_filter(&self, block: BlockHash) -> Result<(Vec<u8>, FilterHeader)> {
        let filter: CoreFilter = self
            .call("getblockfilter", json!([block.to_string(), "basic"]))
            .await?;
        Ok((
            hex::decode(filter.filter).context("invalid RPC filter bytes")?,
            FilterHeader::from_str(&filter.header).context("invalid RPC filter header")?,
        ))
    }
}

#[async_trait]
impl HeaderSource for BitcoinCoreRpc {
    async fn tip(&self) -> Result<ChainTip> {
        #[derive(Deserialize)]
        struct Info {
            blocks: u32,
            bestblockhash: String,
        }
        let info: Info = self.call("getblockchaininfo", json!([])).await?;
        Ok(ChainTip {
            height: info.blocks,
            hash: BlockHash::from_str(&info.bestblockhash).context("invalid RPC tip hash")?,
        })
    }

    async fn header_at_height(&self, height: u32) -> Result<Header> {
        let hash = self.block_hash_at(height).await?;
        let raw: String = self
            .call("getblockheader", json!([hash.to_string(), false]))
            .await?;
        let header: Header =
            consensus::deserialize(&hex::decode(raw).context("invalid RPC header hex")?)
                .context("invalid RPC header encoding")?;
        ensure!(header.block_hash() == hash, "RPC header hash mismatch");
        Ok(header)
    }
}

#[async_trait]
impl FilterSource for BitcoinCoreRpc {
    async fn get_cfheaders(&self, start_h: u32, stop_hash: BlockHash) -> Result<CfHeadersBatch> {
        #[derive(Deserialize)]
        struct HeaderInfo {
            height: u32,
            hash: String,
            confirmations: i64,
        }
        let stop: HeaderInfo = self
            .call("getblockheader", json!([stop_hash.to_string(), true]))
            .await?;
        ensure!(
            BlockHash::from_str(&stop.hash)? == stop_hash && stop.confirmations > 0,
            "RPC stop block is not on the active chain"
        );
        ensure!(
            stop.height >= start_h && stop.height - start_h < 2_000,
            "invalid cfheaders request range"
        );
        let previous = if let Some(height) = start_h.checked_sub(1) {
            self.basic_filter(self.block_hash_at(height).await?)
                .await?
                .1
        } else {
            FilterHeader::all_zeros()
        };
        let mut rolling = previous;
        let mut hashes = Vec::with_capacity((stop.height - start_h + 1) as usize);
        for height in start_h..=stop.height {
            let hash = self.block_hash_at(height).await?;
            if height == stop.height {
                ensure!(
                    hash == stop_hash,
                    "RPC chain changed while fetching cfheaders"
                );
            }
            let (filter, reported_header) = self.basic_filter(hash).await?;
            let filter_hash = FilterHash::hash(&filter);
            rolling = filter_hash.filter_header(&rolling);
            ensure!(
                rolling == reported_header,
                "RPC filter-header linkage mismatch at {height}"
            );
            hashes.push(filter_hash);
        }
        ensure!(
            self.block_hash_at(stop.height).await? == stop_hash,
            "RPC chain changed while fetching cfheaders"
        );
        Ok(CfHeadersBatch {
            filter_type: 0,
            start_height: start_h,
            stop_hash,
            previous_filter_header: previous,
            filter_hashes: hashes,
        })
    }

    async fn get_cfilter(&self, block: BlockHash) -> Result<Vec<u8>> {
        Ok(self.basic_filter(block).await?.0)
    }

    async fn get_block(&self, block: BlockHash) -> Result<Vec<u8>> {
        let raw: String = self.call("getblock", json!([block.to_string(), 0])).await?;
        hex::decode(raw).context("invalid RPC block hex")
    }
}
