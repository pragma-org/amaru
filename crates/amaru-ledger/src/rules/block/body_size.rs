use crate::rules::RuleViolation;
use amaru_kernel::{cbor, HeaderBody, MintedBlock};
pub struct BlockBodySizeMismatch {
    pub supplied: usize,
    pub actual: usize,
}

impl From<BlockBodySizeMismatch> for RuleViolation {
    fn from(value: BlockBodySizeMismatch) -> Self {
        RuleViolation::BlockBodySizeMismatch(value)
    }
}

/// This validation checks that the purported block body size matches the actual block body size.
/// The validation of the bounds happens in the networking layer
pub fn block_body_size_valid(
    block_header: &HeaderBody,
    block: &MintedBlock<'_>,
) -> Result<(), BlockBodySizeMismatch> {
    let bh_size = block_header.block_body_size as usize;
    let actual_block_size = calculate_block_body_size(block);

    if bh_size != actual_block_size {
        Err(BlockBodySizeMismatch {
            supplied: bh_size,
            actual: actual_block_size,
        })
    } else {
        Ok(())
    }
}

fn calculate_block_body_size(block: &MintedBlock<'_>) -> usize {
    // TODO: how should we handle encoding errors here, if at all? They've already been deserialized to the MintedBlock type, which holds the original bytes.
    // In other words this *shouldn't* fail...
    let mut tx_bodies_raw = Vec::new();
    let _ = cbor::encode(&block.transaction_bodies, &mut tx_bodies_raw);

    let mut tx_witness_sets_raw = Vec::new();
    let _ = cbor::encode(&block.transaction_witness_sets, &mut tx_witness_sets_raw);

    let mut auxiliary_data_raw = Vec::new();
    let _ = cbor::encode(&block.auxiliary_data_set, &mut auxiliary_data_raw);

    let mut invalid_transactions_raw = Vec::new();
    let _ = cbor::encode(&block.invalid_transactions, &mut invalid_transactions_raw);

    tx_bodies_raw.len()
        + tx_witness_sets_raw.len()
        + auxiliary_data_raw.len()
        + invalid_transactions_raw.len()
}
