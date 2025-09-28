//! Persistence interfaces and implementations used by the engine
//! (e.g., cfheaders tip and last scanned height).
use async_trait::async_trait;
use bitcoin::BlockHash;

/// Minimal persistence interface. No secrets — just progress markers.
#[async_trait]
pub trait Store: Send + Sync {
    /// Latest verified cfheaders rolling tip `(height, rolling_header_hash)`.
    async fn load_cf_tip(&self) -> anyhow::Result<Option<(u32, BlockHash)>>;

    /// Save latest verified cfheaders rolling tip.
    async fn save_cf_tip(&self, height: u32, cfheader: BlockHash) -> anyhow::Result<()>;

    /// Last height whose *filter* we scanned against our watchlist.
    async fn get_last_scanned(&self) -> anyhow::Result<u32>;

    /// Update last scanned height.
    async fn set_last_scanned(&self, height: u32) -> anyhow::Result<()>;

    /// (Optional) birth height to skip ancient history.
    async fn get_birth_height(&self) -> anyhow::Result<Option<u32>> {
        Ok(None)
    }

    /// Set birth height (optional).
    async fn set_birth_height(&self, _h: u32) -> anyhow::Result<()> {
        Ok(())
    }
}

// submodules / concrete stores live here
pub mod sqlite_store;
pub use sqlite_store::SqliteStore;
