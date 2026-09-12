use anyhow::Result;
use bitcoin::{
    bip158::{BlockFilter, FilterHeader},
    consensus, Block, BlockHash, ScriptBuf,
};
use std::str::FromStr;

#[test]
fn all_official_vectors_match_scripts_and_reference_headers() -> Result<()> {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("data/bip158-testnet-19.json"))?;
    for row in vectors.as_array().unwrap().iter().skip(1) {
        let block: Block = consensus::deserialize(&hex::decode(row[2].as_str().unwrap())?)?;
        let filter = BlockFilter::new(&hex::decode(row[5].as_str().unwrap())?);
        let previous = FilterHeader::from_str(row[4].as_str().unwrap())?;
        let expected = FilterHeader::from_str(row[6].as_str().unwrap())?;
        assert_eq!(
            block.block_hash(),
            BlockHash::from_str(row[1].as_str().unwrap())?
        );
        assert!(block.check_merkle_root());
        assert_eq!(filter.filter_header(&previous), expected);
        let previous_scripts = row[3]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| ScriptBuf::from_bytes(hex::decode(s.as_str().unwrap()).unwrap()));
        let scripts = block
            .txdata
            .iter()
            .flat_map(|tx| tx.output.iter())
            .map(|output| output.script_pubkey.clone())
            .filter(|s| !s.is_op_return())
            .chain(previous_scripts)
            .filter(|s| !s.is_empty());
        for script in scripts {
            assert!(
                filter.match_any(&block.block_hash(), std::iter::once(script.as_bytes()))?,
                "vector at height {}",
                row[0]
            );
        }
    }
    Ok(())
}
