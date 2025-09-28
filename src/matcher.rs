use bitcoin::{bip158::BlockFilter, Address, BlockHash, ScriptBuf};

pub fn filter_matches_any<I>(
    block_hash: BlockHash,
    raw_filter: &[u8],
    scripts: I,
) -> Result<bool, bitcoin::bip158::Error>
where
    I: IntoIterator<Item = ScriptBuf>,
{
    let filter = BlockFilter::new(raw_filter);

    // Own the bytes, then iterate by &[…]
    let query_bytes: Vec<Vec<u8>> = scripts.into_iter().map(|s| s.as_bytes().to_vec()).collect();
    let mut it = query_bytes.iter().map(|v| v.as_slice());

    filter.match_any(&block_hash, &mut it)
}

#[allow(dead_code)]
pub fn filter_matches_any_address<I>(
    block_hash: BlockHash,
    raw_filter: &[u8],
    addrs: I,
) -> Result<bool, bitcoin::bip158::Error>
where
    I: IntoIterator<Item = Address>,
{
    let scripts = addrs.into_iter().map(|a| a.script_pubkey());
    filter_matches_any(block_hash, raw_filter, scripts)
}
