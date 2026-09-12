mod common;
use anyhow::Result;
use bitcoin::{bip158::FilterHeader, hashes::Hash};
use common::*;
use niebla_158::prelude::*;
use rusqlite::Connection;
use tempfile::NamedTempFile;

#[tokio::test]
async fn sqlite_store_roundtrips_across_reopen() -> Result<()> {
    let file = NamedTempFile::new()?;
    let records = Chain::new(&[script(1), script(2)]).records();
    {
        let store = SqliteStore::new(file.path())?;
        assert!(store.load_cf_tip().await?.is_none());
        assert!(store.get_last_scanned().await?.is_none());
        assert!(store.get_birth_height().await?.is_none());
        store.save_filters(&records).await?;
        store.set_last_scanned(Some(2)).await?;
        store.set_birth_height(Some(0)).await?;
    }
    let store = SqliteStore::new(file.path())?;
    assert_eq!(store.load_cf_tip().await?, Some(records[2]));
    assert_eq!(store.load_filter(0).await?, Some(records[0]));
    assert_eq!(store.get_last_scanned().await?, Some(2));
    assert_eq!(store.get_birth_height().await?, Some(0));
    Ok(())
}

#[tokio::test]
async fn memory_store_persists_across_calls_and_clones_but_isolates_instances() -> Result<()> {
    let records = Chain::new(&[]).records();
    let store = SqliteStore::new_in_memory()?;
    store.save_filters(&records).await?;
    store.set_last_scanned(Some(0)).await?;
    let clone = store.clone();
    drop(store);
    assert_eq!(clone.get_last_scanned().await?, Some(0));
    assert_eq!(clone.load_cf_tip().await?, Some(records[0]));
    let separate = SqliteStore::new_in_memory()?;
    assert!(separate.load_cf_tip().await?.is_none());
    assert!(separate.get_last_scanned().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn invalid_appends_are_atomic_and_cannot_overwrite_history() -> Result<()> {
    let records = Chain::new(&[script(1), script(2)]).records();
    let store = SqliteStore::new_in_memory()?;
    store.save_filters(&records[..1]).await?;
    let mut invalid = records[1..].to_vec();
    invalid[1].filter_header = FilterHeader::all_zeros();
    assert!(store.save_filters(&invalid).await.is_err());
    assert_eq!(store.load_cf_tip().await?, Some(records[0]));
    assert!(store.load_filter(1).await?.is_none());
    assert!(store.save_filters(&records[..1]).await.is_err());
    assert!(store.save_filters(&records[2..]).await.is_err());
    assert!(store.save_filters(&[]).await.is_err());
    assert!(store.set_last_scanned(Some(1)).await.is_err());
    store.save_filters(&records[1..]).await?;
    assert_eq!(store.load_cf_tip().await?, Some(records[2]));
    Ok(())
}

#[tokio::test]
async fn rewind_updates_commitments_and_scan_cursor_and_preserves_birth() -> Result<()> {
    let store = SqliteStore::new_in_memory()?;
    let records = Chain::new(&[script(1), script(2)]).records();
    store.save_filters(&records).await?;
    store.set_last_scanned(Some(2)).await?;
    store.set_birth_height(Some(1)).await?;
    assert!(store.rewind(Some(3)).await.is_err());
    assert_eq!(store.load_cf_tip().await?, Some(records[2]));
    store.rewind(Some(0)).await?;
    assert_eq!(store.load_cf_tip().await?, Some(records[0]));
    assert_eq!(store.get_last_scanned().await?, Some(0));
    assert!(store.load_filter(1).await?.is_none());
    store.rewind(None).await?;
    assert!(store.load_cf_tip().await?.is_none());
    assert!(store.get_last_scanned().await?.is_none());
    assert_eq!(store.get_birth_height().await?, Some(1));
    store.set_birth_height(None).await?;
    assert!(store.get_birth_height().await?.is_none());
    Ok(())
}

#[test]
fn legacy_progress_is_rejected_without_overwriting_the_database() -> Result<()> {
    let file = NamedTempFile::new()?;
    let conn = Connection::open(file.path())?;
    conn.execute_batch(
        "CREATE TABLE state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT INTO state VALUES('cf_tip_height','123');
        INSERT INTO state VALUES('last_scanned','100');",
    )?;
    let error = SqliteStore::new(file.path())
        .err()
        .expect("legacy database must be rejected");
    assert!(error.to_string().contains("legacy 0.1"));
    let value: String = conn.query_row(
        "SELECT value FROM state WHERE key='last_scanned'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(value, "100");
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    assert_eq!(version, 0);
    Ok(())
}

#[tokio::test]
async fn corrupt_heights_produce_errors_instead_of_silent_defaults() -> Result<()> {
    let file = NamedTempFile::new()?;
    let store = SqliteStore::new(file.path())?;
    let conn = Connection::open(file.path())?;
    conn.execute_batch("INSERT INTO state VALUES('birth_height','invalid'); INSERT INTO state VALUES('scan_height','invalid');")?;
    assert!(store.get_birth_height().await.is_err());
    assert!(store.get_last_scanned().await.is_err());
    Ok(())
}
