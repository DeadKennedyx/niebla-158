use bitcoin::BlockHash;

/// Return known rolling cfheader checkpoints for a network.
/// For now we return an empty list (no external trust). If you have
/// a vetted list, populate it here (height, rolling_header_hash).
#[allow(dead_code)]
pub fn mainnet_checkpoints() -> Vec<(u32, BlockHash)> {
    vec![]
}
#[allow(dead_code)]
pub fn testnet_checkpoints() -> Vec<(u32, BlockHash)> {
    vec![]
}
#[allow(dead_code)]
pub fn signet_checkpoints() -> Vec<(u32, BlockHash)> {
    vec![]
}
