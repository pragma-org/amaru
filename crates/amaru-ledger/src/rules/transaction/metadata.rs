use amaru_kernel::{AuxiliaryData, Bytes, ComputeHash, Hash, MintedTransactionBody};

use crate::rules::TransactionRuleViolation;

pub enum InvalidTransactionMetadata {
    MissingTransactionMetadata(Bytes),
    MissingTransactionAuxiliaryDataHash(Hash<32>),
    ConflictingMetadataHash {
        supplied: Hash<32>,
        expected: Hash<32>,
    },
}

impl Into<TransactionRuleViolation> for InvalidTransactionMetadata {
    fn into(self) -> TransactionRuleViolation {
        TransactionRuleViolation::InvalidTransactionMetadata(self)
    }
}

pub fn validate_metadata(
    transaction: &MintedTransactionBody<'_>,
    auxilary_data: Option<&AuxiliaryData>,
) -> Result<(), InvalidTransactionMetadata> {
    match (transaction.auxiliary_data_hash.as_ref(), auxilary_data) {
        (None, None) => Ok(()),
        (None, Some(auxiliary_data)) => Err(
            InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(
                auxiliary_data.compute_hash(),
            ),
        ),
        (Some(adh), None) => Err(InvalidTransactionMetadata::MissingTransactionMetadata(
            adh.clone(),
        )),
        (Some(supplied_hash), Some(ad)) => {
            let expected_hash = ad.compute_hash();
            let supplied_hash = Hash::from(&supplied_hash[..]);
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
