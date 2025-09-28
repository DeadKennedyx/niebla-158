use anyhow::{bail, Result};
use bitcoin::{
    hashes::{sha256d, Hash},
    BlockHash,
};

/// Rolling cfheaders chain state:
/// tip_height: last height applied
/// tip_hash: the *rolling* header after applying up to tip_height
///
/// Rolling update formula (BIP157):
///   H_n = HASH256( H_{n-1} || F_n )
/// where F_n is the per-block filter header (HASH256 of the raw filter bytes).
///
/// We verify against optional checkpoints that give H_h at certain heights.
pub struct CfHeaderChain {
    pub tip_height: u32,
    pub tip_hash: BlockHash,
}

impl CfHeaderChain {
    /// Initialize from store (or start at height 0 with H_0 = all-zero).
    pub fn new_from_store(prev: Option<(u32, BlockHash)>) -> Self {
        match prev {
            Some((h, hh)) if h > 0 => Self {
                tip_height: h,
                tip_hash: hh,
            },
            _ => Self {
                tip_height: 0,
                tip_hash: BlockHash::all_zeros(),
            },
        }
    }

    /// Apply a batch of *per-block filter headers* starting at `start_height`.
    /// `headers[i]` corresponds to height `start_height + i`.
    pub fn apply_batch(
        &mut self,
        start_height: u32,
        headers: &[[u8; 32]],
        checkpoints: &[(u32, BlockHash)],
    ) -> Result<()> {
        // Must be the next contiguous chunk
        let expected = self.tip_height.saturating_add(1);
        if start_height != expected {
            bail!("cfheaders batch start mismatch: got {start_height}, expected {expected}");
        }

        let mut rolling = self.tip_hash;

        for (i, fh_bytes) in headers.iter().enumerate() {
            let h = start_height + i as u32;

            // H_n = HASH256( H_{n-1} || F_n )
            let cur = {
                let mut data = Vec::with_capacity(64);
                data.extend_from_slice(rolling.as_ref()); // H_{n-1}
                data.extend_from_slice(fh_bytes); // F_n
                let d = sha256d::Hash::hash(&data);
                BlockHash::from_byte_array(*d.as_ref())
            };

            // Checkpoint verify (if we have one at this height)
            if let Some((_, chk)) = checkpoints.iter().find(|(hh, _)| *hh == h) {
                if &cur != chk {
                    bail!("cfheaders checkpoint mismatch @{}!", h);
                }
            }

            // Advance tip
            rolling = cur;
            self.tip_height = h;
            self.tip_hash = rolling;
        }

        Ok(())
    }
}
