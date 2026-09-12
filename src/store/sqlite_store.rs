//! SQLite persistence with atomic commitments and a retained connection.
use super::{FilterRecord, Store};
use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use bitcoin::{
    bip158::{FilterHash, FilterHeader},
    hashes::Hash,
    BlockHash,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task;

type RawRecord = (u32, Vec<u8>, Vec<u8>, Vec<u8>);

/// Embedded store. Clones share one retained connection, including in memory.
///
/// Opening a 0.1 database containing legacy progress returns an actionable
/// error. Those markers lack block identities and filter commitments and cannot
/// be safely promoted to verified state. Use a new database and replay history.
#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    operation: Arc<tokio::sync::Mutex<()>>,
}

impl SqliteStore {
    /// Open or create an on-disk store with schema version 2.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let conn = Connection::open(&path)
            .with_context(|| format!("open sqlite at {}", path.display()))?;
        Self::initialize(conn)
    }

    /// Create an isolated in-memory store retained until its last handle drops.
    pub fn new_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            version == 0 || version == 2,
            "unsupported SQLite schema version {version}"
        );
        if version == 0 {
            let has_state: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='state')",
                [],
                |row| row.get(0),
            )?;
            if has_state {
                let legacy: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM state WHERE key IN ('cf_tip_height','cf_tip_hash','last_scanned'))",
                    [], |row| row.get(0))?;
                ensure!(!legacy, "legacy 0.1 progress has no authenticated filter commitments; use a new database and reset/replay wallet history");
            }
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS filters (
                height INTEGER PRIMARY KEY CHECK(height BETWEEN 0 AND 4294967295),
                block_hash BLOB NOT NULL CHECK(length(block_hash)=32),
                filter_hash BLOB NOT NULL CHECK(length(filter_hash)=32),
                filter_header BLOB NOT NULL CHECK(length(filter_header)=32)
             );
             PRAGMA user_version=2;",
        )?;
        tx.commit()?;
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            operation: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    async fn with_connection<T, F>(&self, action: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let connection = self.connection.clone();
        // A cancelled caller must not allow a later read/rewind to overtake its
        // still-running blocking write. Keep this guard inside the worker.
        let operation = self.operation.clone().lock_owned().await;
        task::spawn_blocking(move || {
            let _operation = operation;
            let mut conn = connection
                .lock()
                .map_err(|_| anyhow::anyhow!("SQLite connection lock poisoned"))?;
            action(&mut conn)
        })
        .await
        .context("SQLite worker failed")?
    }

    fn record(conn: &Connection, height: Option<u32>) -> Result<Option<FilterRecord>> {
        let read = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RawRecord> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        };
        let raw = match height {
            Some(h) => conn.query_row("SELECT height,block_hash,filter_hash,filter_header FROM filters WHERE height=?1", [h], read).optional()?,
            None => conn.query_row("SELECT height,block_hash,filter_hash,filter_header FROM filters ORDER BY height DESC LIMIT 1", [], read).optional()?,
        };
        raw.map(|(height, block, filter, header)| {
            Ok(FilterRecord {
                height,
                block_hash: BlockHash::from_slice(&block).context("invalid stored block hash")?,
                filter_hash: FilterHash::from_slice(&filter)
                    .context("invalid stored filter hash")?,
                filter_header: FilterHeader::from_slice(&header)
                    .context("invalid stored filter header")?,
            })
        })
        .transpose()
    }

    fn get_height(conn: &Connection, key: &str) -> Result<Option<u32>> {
        let value: Option<String> = conn
            .query_row("SELECT value FROM state WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        value
            .map(|v| v.parse().with_context(|| format!("invalid stored {key}")))
            .transpose()
    }

    fn set_height(conn: &Connection, key: &str, value: Option<u32>) -> Result<()> {
        match value {
            Some(height) => {
                conn.execute("INSERT INTO state(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, height.to_string()])?;
            }
            None => {
                conn.execute("DELETE FROM state WHERE key=?1", [key])?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn load_cf_tip(&self) -> Result<Option<FilterRecord>> {
        self.with_connection(|conn| Self::record(conn, None)).await
    }

    async fn load_filter(&self, height: u32) -> Result<Option<FilterRecord>> {
        self.with_connection(move |conn| Self::record(conn, Some(height)))
            .await
    }

    async fn save_filters(&self, records: &[FilterRecord]) -> Result<()> {
        ensure!(!records.is_empty(), "cannot persist an empty filter batch");
        let records = records.to_vec();
        self.with_connection(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let tip = Self::record(&tx, None)?;
            let next = tip.map(|r| u64::from(r.height) + 1).unwrap_or(0);
            let mut previous = tip.map(|r| r.filter_header).unwrap_or_else(FilterHeader::all_zeros);
            for (expected, record) in (next..).zip(records) {
                ensure!(u64::from(record.height) == expected, "stored filter batch must append contiguously at {expected}");
                ensure!(record.filter_hash.filter_header(&previous) == record.filter_header, "invalid stored filter-header link at {}", record.height);
                tx.execute("INSERT INTO filters(height,block_hash,filter_hash,filter_header) VALUES(?1,?2,?3,?4)",
                    params![record.height, record.block_hash.as_byte_array().as_slice(), record.filter_hash.as_byte_array().as_slice(), record.filter_header.as_byte_array().as_slice()])?;
                previous = record.filter_header;
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    async fn rewind(&self, retained_height: Option<u32>) -> Result<()> {
        self.with_connection(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            match retained_height {
                Some(height) => {
                    ensure!(
                        Self::record(&tx, Some(height))?.is_some(),
                        "rewind target is not stored"
                    );
                    tx.execute("DELETE FROM filters WHERE height>?1", [height])?;
                }
                None => {
                    tx.execute("DELETE FROM filters", [])?;
                }
            }
            let scanned = Self::get_height(&tx, "scan_height")?;
            if scanned > retained_height {
                Self::set_height(&tx, "scan_height", retained_height)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn get_last_scanned(&self) -> Result<Option<u32>> {
        self.with_connection(|conn| Self::get_height(conn, "scan_height"))
            .await
    }

    async fn set_last_scanned(&self, height: Option<u32>) -> Result<()> {
        self.with_connection(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(h) = height {
                ensure!(
                    Self::record(&tx, Some(h))?.is_some(),
                    "cannot scan beyond stored filter commitments"
                );
            }
            Self::set_height(&tx, "scan_height", height)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn get_birth_height(&self) -> Result<Option<u32>> {
        self.with_connection(|conn| Self::get_height(conn, "birth_height"))
            .await
    }

    async fn set_birth_height(&self, height: Option<u32>) -> Result<()> {
        self.with_connection(move |conn| Self::set_height(conn, "birth_height", height))
            .await
    }
}
