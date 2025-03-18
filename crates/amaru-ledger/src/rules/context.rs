use std::collections::BTreeMap;

use amaru_kernel::{Hasher, KeepRaw, MintedTransactionBody, TransactionInput, TransactionOutput};

// A slice here is a subset of the ledger state a ta moment in time
pub type UtxoSlice = BTreeMap<TransactionInput, TransactionOutput>;

// The BlockValidationContext is a collection of slices needed to validate a block
#[derive(Default, Debug)]
pub struct BlockValidationContext {
    pub utxo_slice: UtxoSlice,
    // TODO: add more slices as needed
}

impl BlockValidationContext {
    pub fn update(
        &mut self,
        transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
        transaction_is_valid: bool,
    ) {
        let tx_hash = Hasher::<256>::hash(transaction_body.raw_cbor());
        if transaction_is_valid {
            transaction_body.inputs.iter().for_each(|input| {
                self.utxo_slice.remove(input);
            });

            for (output, index) in transaction_body.outputs.iter().zip(0u64..) {
                self.utxo_slice.insert(
                    TransactionInput {
                        transaction_id: tx_hash,
                        index,
                    },
                    // TODO: Can we not clone here?
                    output.clone().into(),
                );
            }
        } else {
            match &transaction_body.collateral {
                Some(collateral) => {
                    collateral.iter().for_each(|input| {
                        self.utxo_slice.remove(input);
                    });

                    if let Some(collateral_return) = &transaction_body.collateral_return {
                        self.utxo_slice.insert(
                            TransactionInput {
                                transaction_id: tx_hash,
                                // Collateral output index is last_output_index + 1
                                // Safe to do `as u64` here because we won't have 2^32+1 outputs in a tx
                                index: transaction_body.outputs.len() as u64,
                            },
                            collateral_return.clone().into(),
                        );
                    }
                }
                None => {
                    // TODO: This should never be possible to reach, but do we want to error? Or just assume our validation has occurred and we're only updating with valid transactions?
                }
            }
        }
    }
}
