use std::collections::BTreeMap;

use amaru_kernel::{TransactionInput, TransactionOutput};

// A slice here is a subset of the ledger state a ta moment in time
pub type UtxoSlice = BTreeMap<TransactionInput, TransactionOutput>;

// The BlockValidationContext is a collection of slices needed to validate a block
pub struct BlockValidationContext {
    pub utxo_slice: UtxoSlice,
    // TODO: add more slices as needed
}
