#![cfg(feature = "rpc")]
mod common;
use anyhow::Result;
use bitcoin::{consensus, BlockHash};
use common::*;
use niebla_158::{
    prelude::*,
    rpc::{BitcoinCoreRpc, RpcAuth},
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

struct Reply {
    status: u16,
    body: Value,
    location: bool,
    declared_length: Option<usize>,
    delay: Duration,
}
impl Reply {
    fn result(request: &Value, result: Value) -> Self {
        Self {
            status: 200,
            body: json!({"id":request["id"],"result":result,"error":null}),
            location: false,
            declared_length: None,
            delay: Duration::ZERO,
        }
    }
}

struct Server {
    url: String,
    done: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(Value, String) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let stopping = done.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("mock listener: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut length = 0;
                let mut authorization = String::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                        if name.eq_ignore_ascii_case("authorization") {
                            authorization = value.trim().to_owned();
                        }
                    }
                }
                assert!(length < 1_000_000);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request = serde_json::from_slice(&bytes).unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                let reply = handler(request, authorization);
                thread::sleep(reply.delay);
                let body = reply.body.to_string();
                let location = if reply.location {
                    "Location: /redirected\r\n"
                } else {
                    ""
                };
                let response = format!("HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n{body}", reply.status, reply.declared_length.unwrap_or(body.len()));
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            done,
            calls,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
fn auth() -> RpcAuth {
    RpcAuth::UserPass {
        username: "user".into(),
        password: "test-only".into(),
    }
}

#[tokio::test]
async fn rpc_adapter_drives_verified_genesis_scan() -> Result<()> {
    let chain = Chain::new(&[]);
    let block = chain.block(0);
    let hash = block.block_hash();
    let record = chain.records()[0];
    let filter_bytes = filter(&block);
    let server_block = block.clone();
    let server = Server::new(move |request, authorization| {
        assert!(authorization.starts_with("Basic "));
        let result = match request["method"].as_str().unwrap() {
            "getblockchaininfo" => json!({"blocks":0,"bestblockhash":hash.to_string()}),
            "getblockhash" => json!(hash.to_string()),
            "getblockheader" if request["params"][1] == true => {
                json!({"height":0,"hash":hash.to_string(),"confirmations":1})
            }
            "getblockheader" => json!(hex::encode(consensus::serialize(&server_block.header))),
            "getblockfilter" => {
                json!({"filter":hex::encode(&filter_bytes),"header":record.filter_header.to_string()})
            }
            "getblock" => json!(hex::encode(consensus::serialize(&server_block))),
            method => panic!("unexpected RPC {method}"),
        };
        Reply::result(&request, result)
    });
    let rpc = BitcoinCoreRpc::new(&server.url, auth())?;
    let wallet = Wallet::new(vec![block.txdata[0].output[0].script_pubkey.clone()]);
    let progress = Niebla158::new(
        SqliteStore::new_in_memory()?,
        wallet.clone(),
        rpc.clone(),
        rpc,
    )
    .with_trusted_filter_source()
    .run_to_tip()
    .await?;
    assert_eq!(progress.last_scanned, Some(0));
    assert_eq!(wallet.heights(), vec![0]);
    Ok(())
}

#[tokio::test]
async fn rpc_rejects_wrong_ids_errors_redirects_and_oversized_responses() -> Result<()> {
    for case in 0..4 {
        let server = Server::new(move |request, _| {
            let mut reply = Reply::result(
                &request,
                json!({"blocks":0,"bestblockhash":"00".repeat(32)}),
            );
            match case {
                0 => reply.body["id"] = json!(99999),
                1 => reply.body["error"] = json!({"code":-5,"message":"dummy"}),
                2 => {
                    reply.status = 302;
                    reply.location = true;
                }
                3 => reply.declared_length = Some(20 * 1024 * 1024),
                _ => unreachable!(),
            }
            reply
        });
        assert!(BitcoinCoreRpc::new(&server.url, auth())?
            .tip()
            .await
            .is_err());
        assert_eq!(
            server.calls.load(Ordering::SeqCst),
            1,
            "redirects and failures must not trigger retries"
        );
    }
    Ok(())
}

#[tokio::test]
async fn cookie_credentials_are_reread_after_rotation() -> Result<()> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let server = Server::new(move |request, authorization| {
        recorded.lock().unwrap().push(authorization);
        Reply::result(
            &request,
            json!({"blocks":0,"bestblockhash":"00".repeat(32)}),
        )
    });
    let cookie = tempfile::NamedTempFile::new()?;
    std::fs::write(cookie.path(), "user:one\n")?;
    let rpc = BitcoinCoreRpc::new(&server.url, RpcAuth::CookieFile(cookie.path().to_owned()))?;
    rpc.tip().await?;
    std::fs::write(cookie.path(), "user:two\n")?;
    rpc.tip().await?;
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["Basic dXNlcjpvbmU=", "Basic dXNlcjp0d28="]
    );
    Ok(())
}

#[tokio::test]
async fn http_timeout_aborts_the_request() -> Result<()> {
    let server = Server::new(|request, _| {
        let mut reply = Reply::result(&request, json!(null));
        reply.delay = Duration::from_millis(100);
        reply
    });
    let rpc = BitcoinCoreRpc::new(&server.url, auth())?.with_timeout(Duration::from_millis(20));
    assert!(rpc.tip().await.is_err());
    Ok(())
}

#[test]
fn unsafe_rpc_urls_are_rejected_without_network_access() {
    for url in [
        "http://example.com",
        "http://127.0.0.1.example.com",
        "http://user:secret@127.0.0.1",
        "http://127.0.0.1?secret=x",
        "http://127.0.0.1#secret",
    ] {
        assert!(BitcoinCoreRpc::new(url, auth()).is_err());
    }
    for url in ["http://127.0.0.1", "http://[::1]", "https://example.com"] {
        assert!(BitcoinCoreRpc::new(url, auth()).is_ok());
    }
}

#[tokio::test]
async fn orphaned_rpc_stop_block_is_rejected() -> Result<()> {
    let server = Server::new(|request, _| {
        Reply::result(
            &request,
            json!({"height":1,"hash":"00".repeat(32),"confirmations":-1}),
        )
    });
    let rpc = BitcoinCoreRpc::new(&server.url, auth())?;
    let hash: BlockHash = "00".repeat(32).parse()?;
    assert!(rpc.get_cfheaders(0, hash).await.is_err());
    Ok(())
}
