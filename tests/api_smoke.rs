mod common;
use anyhow::Result;
use common::*;
use niebla_158::prelude::*;

#[tokio::test]
async fn engine_compiles_and_runs_with_no_hits() -> Result<()> {
    let chain = Chain::new(&[script(1), script(1)]);
    let wallet = Wallet::new(vec![script(2)]);
    let store = SqliteStore::new_in_memory()?;
    let progress = engine(store.clone(), wallet.clone(), chain.clone())
        .run_to_tip()
        .await?;
    assert_eq!(progress.last_scanned, Some(2));
    assert_eq!(store.load_cf_tip().await?, chain.records().last().copied());
    assert!(wallet.heights().is_empty());
    assert!(chain.data.lock().unwrap().block_requests.is_empty());
    Ok(())
}

#[tokio::test]
async fn genesis_is_synchronized_scanned_and_preserved_on_restart() -> Result<()> {
    let chain = Chain::new(&[]);
    let script = chain.block(0).txdata[0].output[0].script_pubkey.clone();
    let store = SqliteStore::new_in_memory()?;
    let wallet = Wallet::new(vec![script]);
    engine(store.clone(), wallet.clone(), chain.clone())
        .run_to_tip()
        .await?;
    assert_eq!(chain.data.lock().unwrap().batch_requests[0].0, 0);
    assert_eq!(store.get_last_scanned().await?, Some(0));
    assert_eq!(wallet.heights(), vec![0]);
    let expected = store.load_cf_tip().await?;
    engine(store.clone(), wallet.clone(), chain)
        .run_to_tip()
        .await?;
    assert_eq!(expected, store.load_cf_tip().await?);
    assert_eq!(wallet.events.lock().unwrap().len(), 1);
    Ok(())
}
