use amaru_kernel::{AuxiliaryData, Bytes, ComputeHash, Hash, TransactionBody};

use crate::rules::TransactionRuleViolation;

pub enum InvalidTransactionMetadata {
    MissingTransactionMetadata(Bytes),
    MissingTransactionAuxiliaryDataHash(Hash<32>),
    ConflictingMetadataHash {
        supplied: Vec<u8>,
        expected: Vec<u8>,
    },
}

impl Into<TransactionRuleViolation> for InvalidTransactionMetadata {
    fn into(self) -> TransactionRuleViolation {
        TransactionRuleViolation::InvalidTransactionMetadata(self)
    }
}

// TODO clean up clones, introduce lifetimes instead
pub fn validate_metadata(
    transaction: &TransactionBody,
    auxilary_data: Option<AuxiliaryData>,
) -> Result<(), InvalidTransactionMetadata> {
    match (transaction.auxiliary_data_hash.clone(), auxilary_data) {
        (None, None) => Ok(()),
        (None, Some(auxiliary_data)) => Err(
            InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(
                auxiliary_data.compute_hash(),
            ),
        ),
        (Some(adh), None) => Err(InvalidTransactionMetadata::MissingTransactionMetadata(adh)),
        (Some(supplied_hash), Some(ad)) => {
            let expected_hash = ad.compute_hash().as_ref().to_vec();
            let supplied_hash: Vec<u8> = supplied_hash.into();

            if expected_hash != supplied_hash {
                Err(InvalidTransactionMetadata::ConflictingMetadataHash {
                    supplied: supplied_hash,
                    expected: expected_hash,
                })
            } else {
                // the validateMetadata logic is not implemented here (unlike the Haskell code), but instead during deserialization (TODO)
                Ok(())
            }
        }
    }
}
