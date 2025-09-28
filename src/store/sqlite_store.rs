//! Embedded SQLite store implementation for engine progress.
use anyhow::Context;
use async_trait::async_trait;
use bitcoin::BlockHash;
use rusqlite::{params, Connection};
use std::{path::PathBuf, str::FromStr};
use tokio::task;

use crate::store::Store;

/// Simple key/value table:
///   state(key TEXT PRIMARY KEY, value TEXT NOT NULL)
///
/// Keys used:
///  - cf_tip_height  : u32 decimal string
///  - cf_tip_hash    : hex BlockHash
///  - last_scanned   : u32 decimal string
///  - birth_height   : u32 decimal string (optional)
pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    /// Creates/initializes the SQLite file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let conn = Connection::open(&path)
            .with_context(|| format!("open sqlite at {}", path.display()))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;

            CREATE TABLE IF NOT EXISTS state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(Self { path })
    }

    /// Convenient in-memory store (useful for tests)
    pub fn new_in_memory() -> anyhow::Result<Self> {
        let s = Self {
            path: PathBuf::from(":memory:"),
        };
        // Ensure schema exists for in-memory (each open creates a fresh DB)
        let conn = Connection::open(&s.path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(s)
    }

    #[allow(dead_code)]
    fn open(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open(&self.path)?)
    }

    fn kv_get(conn: &Connection, key: &str) -> anyhow::Result<Option<String>> {
        let mut stmt = conn.prepare("SELECT value FROM state WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            let v: String = row.get(0)?;
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }

    fn kv_set(conn: &Connection, key: &str, val: &str) -> anyhow::Result<()> {
        conn.execute(
            "INSERT INTO state(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, val],
        )?;
        Ok(())
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn load_cf_tip(&self) -> anyhow::Result<Option<(u32, BlockHash)>> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            let h = Self::kv_get(&conn, "cf_tip_height")?;
            let hh = Self::kv_get(&conn, "cf_tip_hash")?;
            match (h, hh) {
                (Some(hs), Some(hh)) => {
                    let height: u32 = hs.parse().context("parse cf_tip_height")?;
                    let hash = BlockHash::from_str(&hh).context("parse cf_tip_hash")?;
                    Ok(Some((height, hash)))
                }
                _ => Ok(None),
            }
        })
        .await?
    }

    async fn save_cf_tip(&self, height: u32, cfheader: BlockHash) -> anyhow::Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            let _tx = conn.unchecked_transaction()?;
            Self::kv_set(&conn, "cf_tip_height", &height.to_string())?;
            Self::kv_set(&conn, "cf_tip_hash", &cfheader.to_string())?;
            _tx.commit()?;
            Ok(())
        })
        .await?
    }

    async fn get_last_scanned(&self) -> anyhow::Result<u32> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            Ok(Self::kv_get(&conn, "last_scanned")?
                .as_deref()
                .unwrap_or("0")
                .parse::<u32>()
                .unwrap_or(0))
        })
        .await?
    }

    async fn set_last_scanned(&self, height: u32) -> anyhow::Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            Self::kv_set(&conn, "last_scanned", &height.to_string())
        })
        .await?
    }

    async fn get_birth_height(&self) -> anyhow::Result<Option<u32>> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            Ok(Self::kv_get(&conn, "birth_height")?
                .map(|s| s.parse::<u32>().unwrap_or(0))
                .filter(|&n| n > 0))
        })
        .await?
    }

    async fn set_birth_height(&self, h: u32) -> anyhow::Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            Self::kv_set(&conn, "birth_height", &h.to_string())
        })
        .await?
    }
}
