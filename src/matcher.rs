use bitcoin::{bip158::BlockFilter, BlockHash, ScriptBuf};

pub(crate) fn filter_matches_any(
    block_hash: BlockHash,
    raw_filter: &[u8],
    scripts: &[ScriptBuf],
) -> Result<bool, bitcoin::bip158::Error> {
    BlockFilter::new(raw_filter)
        .match_any(&block_hash, scripts.iter().map(|script| script.as_bytes()))
}
