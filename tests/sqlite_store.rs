use bitcoin::hashes::{sha256d, Hash};
use bitcoin::BlockHash;
use niebla_158::store::{sqlite_store::SqliteStore, Store}; // bring trait methods into scope // for all_zeros() + from_raw_hash()

use tempfile::NamedTempFile;

#[tokio::test]
async fn sqlite_store_roundtrips() -> anyhow::Result<()> {
    // temp file for each run
    let tmp = NamedTempFile::new()?;
    let path = tmp.path().to_string_lossy().to_string();

    let store = SqliteStore::new(&path)?;

    // Defaults on a fresh DB
    let cf_tip = store.load_cf_tip().await?;
    assert!(
        cf_tip.is_none(),
        "fresh DB should have no cfheaders tip yet"
    );

    let last_scanned = store.get_last_scanned().await?;
    assert_eq!(last_scanned, 0, "fresh DB starts at last_scanned=0");

    let birth = store.get_birth_height().await?;
    assert!(
        birth.is_none(),
        "birth height is optional and unset by default"
    );

    // Persist values and round-trip them
    let h = 123_456u32;
    let cf = BlockHash::from_raw_hash(sha256d::Hash::all_zeros()); // any 32-byte value is fine here
    store.save_cf_tip(h, cf).await?;
    assert_eq!(store.load_cf_tip().await?, Some((h, cf)));

    store.set_last_scanned(h).await?;
    assert_eq!(store.get_last_scanned().await?, h);

    store.set_birth_height(200_000).await?;
    assert_eq!(store.get_birth_height().await?, Some(200_000));

    Ok(())
}
