mod common;
use anyhow::Result;
use bitcoin::{bip158::FilterHeader, consensus, hashes::Hash, Amount, BlockHash, Witness};
use common::*;
use niebla_158::prelude::*;
use std::{sync::atomic::Ordering, time::Duration};

#[tokio::test]
async fn engine_hits_when_filter_contains_watch_script() -> Result<()> {
    let chain = Chain::new(&[script(7)]);
    let wallet = Wallet::new(vec![script(7)]);
    let store = SqliteStore::new_in_memory()?;
    engine(store.clone(), wallet.clone(), chain.clone())
        .run_to_tip()
        .await?;
    assert_eq!(wallet.heights(), vec![1]);
    assert_eq!(wallet.active.lock().unwrap()[&1], chain.hash(1));
    assert_eq!(store.get_last_scanned().await?, Some(1));
    Ok(())
}

#[tokio::test]
async fn substituted_filter_is_rejected_even_after_trusted_checkpoint() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let store = SqliteStore::new_in_memory()?;
    store.save_filters(&chain.records()).await?;
    store.set_last_scanned(Some(0)).await?;
    let hash = chain.hash(1);
    chain
        .data
        .lock()
        .unwrap()
        .served_filters
        .insert(hash, vec![0]);
    let wallet = Wallet::new(vec![script(1)]);
    let checkpoint = chain.records()[1].filter_header;
    let result = engine(store.clone(), wallet.clone(), chain)
        .with_checkpoints(vec![(1, checkpoint)])
        .run_to_tip()
        .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("filter commitment mismatch"));
    assert_eq!(store.get_last_scanned().await?, Some(0));
    assert!(wallet.heights().is_empty());
    Ok(())
}

#[tokio::test]
async fn malformed_committed_filter_does_not_advance_progress() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    chain.data.lock().unwrap().filters[1] = vec![1];
    let store = SqliteStore::new_in_memory()?;
    let result = engine(store.clone(), Wallet::new(vec![script(1)]), chain)
        .run_to_tip()
        .await;
    assert!(result.unwrap_err().to_string().contains("decode filter"));
    assert_eq!(store.get_last_scanned().await?, Some(0));
    Ok(())
}

#[tokio::test]
async fn unrelated_block_and_tampered_transactions_are_rejected() -> Result<()> {
    for wrong_hash in [true, false] {
        let chain = Chain::new(&[script(1)]);
        let mut bad = if wrong_hash {
            chain.block(0)
        } else {
            chain.block(1)
        };
        if !wrong_hash {
            bad.txdata[0].output[0].value = Amount::from_sat(1);
        }
        let hash = chain.hash(1);
        chain
            .data
            .lock()
            .unwrap()
            .served_blocks
            .insert(hash, consensus::serialize(&bad));
        let store = SqliteStore::new_in_memory()?;
        let wallet = Wallet::new(vec![script(1)]);
        let error = engine(store.clone(), wallet.clone(), chain)
            .run_to_tip()
            .await
            .unwrap_err();
        assert!(error.to_string().contains(if wrong_hash {
            "block hash mismatch"
        } else {
            "Merkle root mismatch"
        }));
        assert!(wallet.heights().is_empty());
        assert_eq!(store.get_last_scanned().await?, Some(0));
    }
    Ok(())
}

#[tokio::test]
async fn witness_tampering_is_rejected_without_changing_block_hash_or_merkle_root() -> Result<()> {
    let initial = Chain::new(&[script(1)]);
    let mut block = initial.block(1);
    block.txdata[0].input[0].witness = Witness::from_slice(&[[0u8; 32]]);
    let commitment =
        bitcoin::Block::compute_witness_commitment(&block.witness_root().unwrap(), &[0; 32]);
    let mut script_bytes = vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed];
    script_bytes.extend_from_slice(commitment.as_byte_array());
    block.txdata[0].output.push(bitcoin::TxOut {
        value: Amount::ZERO,
        script_pubkey: bitcoin::ScriptBuf::from_bytes(script_bytes),
    });
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    assert!(block.check_witness_commitment());
    let chain = Chain::from_blocks(vec![initial.block(0), block.clone()]);
    let store = SqliteStore::new_in_memory()?;
    engine(store, Wallet::new(vec![script(1)]), chain.clone())
        .run_to_tip()
        .await?;
    block.txdata[0].input[0].witness = Witness::from_slice(&[[1u8; 32]]);
    assert!(block.check_merkle_root());
    assert!(!block.check_witness_commitment());
    let hash = chain.hash(1);
    chain
        .data
        .lock()
        .unwrap()
        .served_blocks
        .insert(hash, consensus::serialize(&block));
    let wallet = Wallet::new(vec![script(1)]);
    let error = engine(SqliteStore::new_in_memory()?, wallet.clone(), chain)
        .run_to_tip()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("witness commitment mismatch"));
    assert!(wallet.heights().is_empty());
    Ok(())
}

#[tokio::test]
async fn malformed_cfheaders_responses_are_rejected_before_persistence() -> Result<()> {
    for fault in [
        BatchFault::Empty,
        BatchFault::Short,
        BatchFault::Long,
        BatchFault::Start,
        BatchFault::Stop,
        BatchFault::Type,
        BatchFault::Previous,
    ] {
        let chain = Chain::new(&[script(1)]);
        chain.data.lock().unwrap().batch_fault = Some(fault);
        let store = SqliteStore::new_in_memory()?;
        assert!(engine(store.clone(), Wallet::default(), chain.clone())
            .run_to_tip()
            .await
            .is_err());
        assert!(store.load_cf_tip().await?.is_none());
        assert!(store.get_last_scanned().await?.is_none());
        assert_eq!(chain.data.lock().unwrap().batch_requests.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn requests_respect_two_thousand_header_limit() -> Result<()> {
    let chain = Chain::new(&vec![script(1); 2_001]);
    let store = SqliteStore::new_in_memory()?;
    engine(store.clone(), Wallet::default(), chain.clone())
        .run_to_tip()
        .await?;
    {
        let data = chain.data.lock().unwrap();
        assert_eq!(
            data.batch_requests,
            vec![
                (0, data.blocks[1999].block_hash()),
                (2000, data.blocks[2001].block_hash())
            ]
        );
    }
    assert_eq!(store.load_cf_tip().await?.unwrap().height, 2001);
    assert!(store.get_last_scanned().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn same_height_reorg_rolls_back_before_replaying_replacement() -> Result<()> {
    let chain = Chain::new(&[script(1), script(1)]);
    let wallet = Wallet::new(vec![script(1), script(2)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    engine.run_to_tip().await?;
    let old_hash = chain.hash(2);
    chain.data.lock().unwrap().replace_from(2, script(2));
    engine.run_to_tip().await?;
    assert_ne!(old_hash, chain.hash(2));
    assert_eq!(wallet.active.lock().unwrap()[&2], chain.hash(2));
    let events = wallet.events.lock().unwrap().clone();
    assert_eq!(
        &events[2..],
        &[Event::Rollback(Some(1)), Event::Match(2, chain.hash(2))]
    );
    assert_eq!(
        store.load_cf_tip().await?.unwrap().block_hash,
        chain.hash(2)
    );
    Ok(())
}

#[tokio::test]
async fn shorter_chain_rewinds_both_cursors() -> Result<()> {
    let chain = Chain::new(&[script(1), script(1)]);
    let wallet = Wallet::new(vec![script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    engine.run_to_tip().await?;
    chain.data.lock().unwrap().blocks.truncate(2);
    chain.data.lock().unwrap().filters.truncate(2);
    let progress = engine.run_to_tip().await?;
    assert_eq!(progress.last_scanned, Some(1));
    assert_eq!(store.load_cf_tip().await?.unwrap().height, 1);
    assert!(store.load_filter(2).await?.is_none());
    assert_eq!(wallet.heights(), vec![1]);
    Ok(())
}

#[tokio::test]
async fn failed_rollback_preserves_old_state_and_can_retry() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let wallet = Wallet::new(vec![script(1), script(2)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    engine.run_to_tip().await?;
    let old_tip = store.load_cf_tip().await?;
    chain.data.lock().unwrap().replace_from(1, script(2));
    wallet.fail_rollback.store(true, Ordering::SeqCst);
    assert!(engine.run_to_tip().await.is_err());
    assert_eq!(store.load_cf_tip().await?, old_tip);
    wallet.fail_rollback.store(false, Ordering::SeqCst);
    engine.run_to_tip().await?;
    assert_eq!(wallet.active.lock().unwrap()[&1], chain.hash(1));
    Ok(())
}

#[tokio::test]
async fn chain_change_during_scan_stops_and_reconciles_on_retry() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let wallet = Wallet::new(vec![script(1), script(2)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    chain.data.lock().unwrap().reorg_on_filter = Some((1, script(2)));
    assert!(engine.run_to_tip().await.is_err());
    assert!(store.get_last_scanned().await?.is_none());
    assert!(wallet.heights().is_empty());
    engine.run_to_tip().await?;
    assert_eq!(wallet.active.lock().unwrap()[&1], chain.hash(1));
    Ok(())
}

#[tokio::test]
async fn birthday_skips_old_filters_but_still_verifies_genesis_history() -> Result<()> {
    let chain = Chain::new(&[script(1), script(1)]);
    let store = SqliteStore::new_in_memory()?;
    store.set_birth_height(Some(2)).await?;
    let wallet = Wallet::new(vec![script(1)]);
    engine(store.clone(), wallet.clone(), chain.clone())
        .run_to_tip()
        .await?;
    let expected_hash = chain.hash(2);
    assert_eq!(
        chain.data.lock().unwrap().filter_requests,
        vec![expected_hash]
    );
    assert!(store.load_filter(0).await?.is_some());
    assert_eq!(wallet.heights(), vec![2]);
    Ok(())
}

#[tokio::test]
async fn watchlist_growth_matches_next_block_and_empty_watchlist_does_not_skip_history(
) -> Result<()> {
    let chain = Chain::new(&[script(1), script(2)]);
    let wallet = Wallet::default();
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    engine.run_to_tip().await?;
    assert!(store.get_last_scanned().await?.is_none());
    assert!(chain.data.lock().unwrap().filter_requests.is_empty());
    wallet.watch.lock().unwrap().push(script(1));
    *wallet.expand.lock().unwrap() = Some(script(2));
    engine.run_to_tip().await?;
    assert_eq!(wallet.heights(), vec![1, 2]);
    Ok(())
}

#[tokio::test]
async fn explicit_rescan_finds_imported_script_and_overrides_birthday() -> Result<()> {
    let chain = Chain::new(&[script(1), script(2)]);
    let wallet = Wallet::new(vec![script(2)]);
    let store = SqliteStore::new_in_memory()?;
    store.set_birth_height(Some(2)).await?;
    let engine = engine(store.clone(), wallet.clone(), chain.clone());
    engine.run_to_tip().await?;
    assert_eq!(wallet.heights(), vec![2]);
    wallet.watch.lock().unwrap().push(script(1));
    engine.rescan_from(0).await?;
    assert_eq!(wallet.heights(), vec![1, 2]);
    assert_eq!(store.get_birth_height().await?, Some(2));
    assert!(engine.rescan_from(3).await.is_err());
    Ok(())
}

#[tokio::test]
async fn callback_failure_does_not_skip_block_on_retry() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let wallet = Wallet::new(vec![script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = engine(store.clone(), wallet.clone(), chain);
    wallet.fail_match.store(true, Ordering::SeqCst);
    assert!(engine.run_to_tip().await.is_err());
    assert_eq!(store.get_last_scanned().await?, Some(0));
    wallet.fail_match.store(false, Ordering::SeqCst);
    engine.run_to_tip().await?;
    assert_eq!(wallet.heights(), vec![1]);
    Ok(())
}

#[tokio::test]
async fn trust_policy_is_required_and_checkpoints_limit_scanning() -> Result<()> {
    let chain = Chain::new(&[script(1), script(1)]);
    let wallet = Wallet::new(vec![script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let plain = Niebla158::new(store.clone(), wallet.clone(), chain.clone(), chain.clone());
    assert!(plain
        .run_to_tip()
        .await
        .unwrap_err()
        .to_string()
        .contains("configure trusted"));
    let checkpoint = chain.records()[1].filter_header;
    let progress = plain
        .with_checkpoints(vec![(1, checkpoint)])
        .run_to_tip()
        .await?;
    assert_eq!(progress.chain_tip.height, 2);
    assert_eq!(progress.verified_tip.height, 1);
    assert_eq!(progress.last_scanned, Some(1));
    assert!(store.load_filter(2).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn checkpoint_mismatches_and_duplicate_heights_are_rejected() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let bad = FilterHeader::all_zeros();
    assert!(engine(store.clone(), Wallet::default(), chain.clone())
        .with_checkpoints(vec![(1, bad)])
        .run_to_tip()
        .await
        .is_err());
    assert!(store.load_cf_tip().await?.is_none());
    engine(store.clone(), Wallet::default(), chain.clone())
        .run_to_tip()
        .await?;
    assert!(engine(store.clone(), Wallet::default(), chain.clone())
        .with_checkpoints(vec![(1, bad)])
        .run_to_tip()
        .await
        .is_err());
    assert!(engine(store, Wallet::default(), chain)
        .with_checkpoints(vec![(1, bad), (1, bad)])
        .run_to_tip()
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn future_checkpoint_does_not_authenticate_partial_prefix() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = Niebla158::new(
        store.clone(),
        Wallet::new(vec![script(1)]),
        chain.clone(),
        chain,
    )
    .with_checkpoints(vec![(2, FilterHeader::all_zeros())]);
    assert!(engine.run_to_tip().await.is_err());
    assert!(store.load_cf_tip().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn network_mismatch_and_broken_header_links_are_rejected() -> Result<()> {
    let store = SqliteStore::new_in_memory()?;
    let chain = Chain::new(&[]);
    engine(store.clone(), Wallet::default(), chain)
        .run_to_tip()
        .await?;
    let other = Chain::from_blocks(vec![bitcoin::blockdata::constants::genesis_block(
        bitcoin::Network::Bitcoin,
    )]);
    let error = engine(store, Wallet::default(), other)
        .run_to_tip()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("different Bitcoin network"));
    let chain = Chain::new(&[script(1)]);
    chain.data.lock().unwrap().blocks[1].header.prev_blockhash = BlockHash::all_zeros();
    assert!(
        engine(SqliteStore::new_in_memory()?, Wallet::default(), chain)
            .run_to_tip()
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn a_nonresponding_source_times_out_without_progress() -> Result<()> {
    let chain = Chain::new(&[]);
    chain.hang_cfheaders.store(true, Ordering::SeqCst);
    let store = SqliteStore::new_in_memory()?;
    let error = engine(store.clone(), Wallet::default(), chain)
        .with_request_timeout(Duration::from_millis(20))
        .run_to_tip()
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    assert!(store.load_cf_tip().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn duplicate_transaction_merkle_ambiguity_is_rejected() -> Result<()> {
    let base = Chain::new(&[script(1)]);
    let mut original = base.block(1);
    for tag in [2u8, 3] {
        let mut tx = original.txdata[0].clone();
        tx.input[0].previous_output = bitcoin::OutPoint {
            txid: bitcoin::Txid::from_byte_array([tag; 32]),
            vout: 0,
        };
        original.txdata.push(tx);
    }
    original.header.merkle_root = original.compute_merkle_root().unwrap();
    let chain = Chain::from_blocks(vec![base.block(0), original.clone()]);
    let mut duplicate = original.clone();
    duplicate
        .txdata
        .push(original.txdata.last().unwrap().clone());
    assert_eq!(duplicate.block_hash(), original.block_hash());
    assert!(
        duplicate.check_merkle_root(),
        "the duplicate preserves the Merkle root"
    );
    chain
        .data
        .lock()
        .unwrap()
        .served_blocks
        .insert(original.block_hash(), consensus::serialize(&duplicate));
    let wallet = Wallet::new(vec![script(1)]);
    let error = engine(SqliteStore::new_in_memory()?, wallet.clone(), chain)
        .run_to_tip()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("duplicate transaction"));
    assert!(wallet.heights().is_empty());
    Ok(())
}

#[tokio::test]
async fn failed_later_checkpoint_never_scans_an_unanchored_saved_prefix() -> Result<()> {
    let chain = Chain::new(&vec![script(1); 2000]);
    let wallet = Wallet::new(vec![script(1)]);
    let store = SqliteStore::new_in_memory()?;
    let engine = Niebla158::new(store.clone(), wallet.clone(), chain.clone(), chain.clone())
        .with_checkpoints(vec![(2000, FilterHeader::all_zeros())]);
    assert!(engine.run_to_tip().await.is_err());
    assert_eq!(store.load_cf_tip().await?.unwrap().height, 1999);
    assert!(store.get_last_scanned().await?.is_none());
    assert!(chain.data.lock().unwrap().filter_requests.is_empty());
    assert!(engine.run_to_tip().await.is_err());
    assert!(wallet.heights().is_empty());
    Ok(())
}

struct FailScanWrite {
    inner: SqliteStore,
    fail: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl Store for FailScanWrite {
    async fn load_cf_tip(&self) -> Result<Option<FilterRecord>> {
        self.inner.load_cf_tip().await
    }
    async fn load_filter(&self, height: u32) -> Result<Option<FilterRecord>> {
        self.inner.load_filter(height).await
    }
    async fn save_filters(&self, records: &[FilterRecord]) -> Result<()> {
        self.inner.save_filters(records).await
    }
    async fn rewind(&self, height: Option<u32>) -> Result<()> {
        self.inner.rewind(height).await
    }
    async fn get_last_scanned(&self) -> Result<Option<u32>> {
        self.inner.get_last_scanned().await
    }
    async fn set_last_scanned(&self, height: Option<u32>) -> Result<()> {
        if height == Some(1) && self.fail.swap(false, Ordering::SeqCst) {
            anyhow::bail!("injected progress write failure");
        }
        self.inner.set_last_scanned(height).await
    }
    async fn get_birth_height(&self) -> Result<Option<u32>> {
        self.inner.get_birth_height().await
    }
    async fn set_birth_height(&self, height: Option<u32>) -> Result<()> {
        self.inner.set_birth_height(height).await
    }
}

#[tokio::test]
async fn reorg_removes_callback_effects_even_when_scan_write_failed() -> Result<()> {
    let chain = Chain::new(&[script(1)]);
    let wallet = Wallet::new(vec![script(1), script(2)]);
    let store = SqliteStore::new_in_memory()?;
    let failing = FailScanWrite {
        inner: store.clone(),
        fail: std::sync::atomic::AtomicBool::new(true),
    };
    let engine = Niebla158::new(failing, wallet.clone(), chain.clone(), chain.clone())
        .with_trusted_filter_source();
    assert!(engine.run_to_tip().await.is_err());
    assert_eq!(wallet.heights(), vec![1]);
    assert_eq!(store.get_last_scanned().await?, Some(0));
    let old = wallet.active.lock().unwrap()[&1];
    chain.data.lock().unwrap().replace_from(1, script(2));
    engine.run_to_tip().await?;
    assert_ne!(wallet.active.lock().unwrap()[&1], old);
    assert!(wallet
        .events
        .lock()
        .unwrap()
        .contains(&Event::Rollback(Some(0))));
    assert_eq!(store.get_last_scanned().await?, Some(1));
    Ok(())
}
