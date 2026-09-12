#![cfg(feature = "rpc")]
mod common;
use anyhow::{ensure, Context, Result};
use bitcoin::{Address, Network};
use common::*;
use niebla_158::{
    prelude::*,
    rpc::{BitcoinCoreRpc, RpcAuth},
};
use serde_json::{json, Value};
use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tempfile::TempDir;

struct Node {
    child: Child,
    directory: TempDir,
}
impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn rpc_call(url: &str, cookie: &Path, method: &str, params: Value) -> Result<Value> {
    let auth = tokio::fs::read_to_string(cookie).await?;
    let (user, password) = auth.trim_end().split_once(':').context("cookie format")?;
    let result: Value = reqwest::Client::new()
        .post(url)
        .basic_auth(user, Some(password))
        .timeout(Duration::from_secs(5))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await?
        .json()
        .await?;
    ensure!(
        result["error"].is_null(),
        "test RPC {method} failed: {}",
        result["error"]
    );
    Ok(result["result"].clone())
}

async fn wait_for_filter(rpc: &BitcoinCoreRpc) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(tip) = rpc.tip().await {
                if rpc.get_cfilter(tip.hash).await.is_ok() {
                    return Ok::<(), anyhow::Error>(());
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("regtest filter index did not become ready")?
}

#[tokio::test]
#[ignore = "requires BITCOIND; starts an isolated node with network activity disabled"]
async fn real_bitcoin_core_sync_restart_and_reorg() -> Result<()> {
    let executable =
        std::env::var_os("BITCOIND").context("set BITCOIND to a Bitcoin Core executable")?;
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let child = Command::new(executable)
        .arg(format!("-datadir={}", directory.path().display()))
        .args([
            "-regtest",
            "-server=1",
            "-listen=0",
            "-networkactive=0",
            "-blockfilterindex=1",
            "-printtoconsole=0",
        ])
        .arg(format!("-rpcport={port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start isolated bitcoind")?;
    let mut node = Node { child, directory };
    let cookie = node.directory.path().join("regtest/.cookie");
    let url = format!("http://127.0.0.1:{port}");
    let rpc = BitcoinCoreRpc::new(&url, RpcAuth::CookieFile(cookie.clone()))?;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            ensure!(
                node.child.try_wait()?.is_none(),
                "bitcoind exited during startup"
            );
            if rpc.tip().await.is_ok() {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("bitcoind startup timed out")??;
    let address_a = Address::from_script(&script(7), Network::Regtest)?;
    let address_b = Address::from_script(&script(8), Network::Regtest)?;
    rpc_call(
        &url,
        &cookie,
        "generatetoaddress",
        json!([3, address_a.to_string()]),
    )
    .await?;
    wait_for_filter(&rpc).await?;
    let path = node.directory.path().join("scan.sqlite");
    let wallet = Wallet::new(vec![script(7), script(8)]);
    let store = SqliteStore::new(&path)?;
    let engine = Niebla158::new(store.clone(), wallet.clone(), rpc.clone(), rpc.clone())
        .with_trusted_filter_source();
    engine.run_to_tip().await?;
    assert_eq!(wallet.heights(), vec![1, 2, 3]);
    let old_two = store.load_filter(2).await?.unwrap().block_hash;
    drop(engine);
    drop(store);
    let reopened = SqliteStore::new(&path)?;
    let engine = Niebla158::new(reopened.clone(), wallet.clone(), rpc.clone(), rpc.clone())
        .with_trusted_filter_source();
    engine.run_to_tip().await?;
    assert_eq!(
        wallet.events.lock().unwrap().len(),
        3,
        "restart must not redeliver committed matches"
    );
    rpc_call(
        &url,
        &cookie,
        "invalidateblock",
        json!([old_two.to_string()]),
    )
    .await?;
    rpc_call(
        &url,
        &cookie,
        "generatetoaddress",
        json!([2, address_b.to_string()]),
    )
    .await?;
    wait_for_filter(&rpc).await?;
    engine.run_to_tip().await?;
    let new_two = reopened.load_filter(2).await?.unwrap().block_hash;
    assert_ne!(old_two, new_two);
    assert_eq!(wallet.active.lock().unwrap()[&2], new_two);
    assert_eq!(reopened.get_last_scanned().await?, Some(3));
    assert!(wallet
        .events
        .lock()
        .unwrap()
        .contains(&Event::Rollback(Some(1))));
    rpc_call(&url, &cookie, "stop", json!([])).await?;
    Ok(())
}
