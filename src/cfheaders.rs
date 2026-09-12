use anyhow::{ensure, Context, Result};
use bitcoin::{
    bip158::{FilterHash, FilterHeader},
    hashes::Hash,
};
use std::collections::BTreeMap;

/// An absent tip is the predecessor of genesis, not a zero-valued height zero.
pub(crate) struct CfHeaderChain {
    pub tip: Option<(u32, FilterHeader)>,
}

impl CfHeaderChain {
    pub fn new(tip: Option<(u32, FilterHeader)>) -> Self {
        Self { tip }
    }

    /// Verify the whole batch before advancing this in-memory tip.
    pub fn apply_batch(
        &mut self,
        start: u32,
        previous: FilterHeader,
        hashes: &[FilterHash],
        checkpoints: &BTreeMap<u32, FilterHeader>,
    ) -> Result<Vec<FilterHeader>> {
        ensure!(
            !hashes.is_empty() && hashes.len() <= 2_000,
            "invalid cfheaders batch length"
        );
        let next = match self.tip {
            Some((height, _)) => height.checked_add(1).context("cfheaders height overflow")?,
            None => 0,
        };
        ensure!(
            start == next,
            "cfheaders start mismatch: got {start}, expected {next}"
        );
        let mut rolling = self
            .tip
            .map(|(_, hash)| hash)
            .unwrap_or_else(FilterHeader::all_zeros);
        ensure!(
            previous == rolling,
            "cfheaders predecessor mismatch at {start}"
        );
        let mut headers = Vec::with_capacity(hashes.len());
        let mut last = start;
        for (i, hash) in hashes.iter().enumerate() {
            last = start
                .checked_add(u32::try_from(i)?)
                .context("cfheaders height overflow")?;
            // BIP-157: SHA256d(filter_hash || previous_filter_header).
            rolling = hash.filter_header(&rolling);
            if let Some(expected) = checkpoints.get(&last) {
                ensure!(
                    *expected == rolling,
                    "cfheaders checkpoint mismatch at {last}"
                );
            }
            headers.push(rolling);
        }
        self.tip = Some((last, rolling));
        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn official_vectors_verify_including_genesis_and_nonzero_predecessors() -> Result<()> {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/bip158-testnet-19.json"))?;
        for row in vectors.as_array().unwrap().iter().skip(1) {
            let height = row[0].as_u64().unwrap() as u32;
            let previous = FilterHeader::from_str(row[4].as_str().unwrap())?;
            let expected = FilterHeader::from_str(row[6].as_str().unwrap())?;
            let hash = FilterHash::hash(&hex::decode(row[5].as_str().unwrap())?);
            let mut chain = CfHeaderChain::new(height.checked_sub(1).map(|h| (h, previous)));
            let headers = chain.apply_batch(
                height,
                previous,
                &[hash],
                &BTreeMap::from([(height, expected)]),
            )?;
            assert_eq!(headers, vec![expected]);
            assert_eq!(chain.tip, Some((height, expected)));
            assert_eq!(CfHeaderChain::new(chain.tip).tip, chain.tip);
        }
        Ok(())
    }

    #[test]
    fn rejected_batch_keeps_original_tip_and_height_overflow_is_an_error() {
        let mut chain = CfHeaderChain::new(None);
        let hashes = [FilterHash::hash(&[0]), FilterHash::hash(&[1])];
        let bad_checkpoint = BTreeMap::from([(1, FilterHeader::all_zeros())]);
        assert!(chain
            .apply_batch(0, FilterHeader::all_zeros(), &hashes, &bad_checkpoint)
            .is_err());
        assert!(chain.tip.is_none());
        assert!(chain
            .apply_batch(0, FilterHeader::all_zeros(), &[], &BTreeMap::new())
            .is_err());
        let mut exhausted = CfHeaderChain::new(Some((u32::MAX, FilterHeader::all_zeros())));
        assert!(exhausted
            .apply_batch(
                u32::MAX,
                FilterHeader::all_zeros(),
                &hashes[..1],
                &BTreeMap::new()
            )
            .is_err());
    }
}
