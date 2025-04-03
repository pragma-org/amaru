// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use amaru_kernel::{AuxiliaryData, Bytes, ComputeHash, Hash, MintedTransactionBody};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum InvalidTransactionMetadata {
    #[error("missing metadata: auxiliary data hash {0}")]
    MissingTransactionMetadata(Bytes),

    #[error("missing auxiliary data hash: metadata hash {0}")]
    MissingTransactionAuxiliaryDataHash(Hash<32>),

    #[error("metadata hash mismatch: supplied {supplied:?} expected {expected:?}")]
    ConflictingMetadataHash {
        supplied: Hash<32>,
        expected: Hash<32>,
    },
}

pub fn execute(
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

#[cfg(test)]
mod tests {
    use crate::tests::include_transaction_body;
    use amaru_kernel::{cbor, from_cbor, AuxiliaryData, Hash, KeepRaw, MintedTransactionBody};

    use super::InvalidTransactionMetadata;

    macro_rules! include_auxiliary_data {
        ($hash:literal) => {
            from_cbor::<AuxiliaryData>(include_bytes!(concat!(
                "../../../tests/data/transactions/preprod/",
                $hash,
                "/auxiliary-data.cbor"
            )))
            .unwrap()
        };
        ($hash:literal, $test_variant:literal) => {
            from_cbor::<AuxiliaryData>(include_bytes!(concat!(
                "../../../tests/data/transactions/preprod/",
                $hash,
                "/",
                $test_variant,
                "/auxiliary_data.cbor"
            )))
            .unwrap()
        };
    }

    #[test]
    fn valid_metadata() {
        let tx_body = include_transaction_body!(
            "../../../tests",
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7"
        );
        let auxiliary_data = include_auxiliary_data!(
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7"
        );

        let result = super::execute(&tx_body, Some(&auxiliary_data));
        assert!(result.is_ok());
    }

    #[test]
    fn missing_auxiliary_data_hash() {
        let tx_body = include_transaction_body!(
            "../../../tests",
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7",
            "missing-adh"
        );
        let auxiliary_data = include_auxiliary_data!(
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7"
        );
        let expected_adh: Hash<32> = Hash::from(*include_bytes!("../../../tests/data/transactions/preprod/a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7/adh.bytes"));

        let result = super::execute(&tx_body, Some(&auxiliary_data));
        match result {
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(hash)) => {
                assert_eq!(hash, expected_adh);
            }
            Err(_) => panic!("Expected MissingTransactionAuxiliaryDataHash error"),
        }
    }

    #[test]
    fn missing_auxiliary_data() {
        let tx_body = include_transaction_body!(
            "../../../tests",
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7"
        );

        let result = super::execute(&tx_body, None);
        match result {
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(InvalidTransactionMetadata::MissingTransactionMetadata(_)) => {}
            Err(_) => panic!("Expected MissingTransactionMetadata error"),
        }
    }

    #[test]
    fn conflicting_auxiliary_data_hash() {
        let tx_body = include_transaction_body!(
            "../../../tests",
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7",
            "conflicting-adh"
        );

        let auxiliary_data = include_auxiliary_data!(
            "a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7"
        );

        let result = super::execute(&tx_body, Some(&auxiliary_data));
        match result {
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(InvalidTransactionMetadata::ConflictingMetadataHash {
                supplied: _,
                expected: _,
            }) => {}
            Err(_) => panic!("Expected MissingTransactionMetadata error"),
        }
    }
}
