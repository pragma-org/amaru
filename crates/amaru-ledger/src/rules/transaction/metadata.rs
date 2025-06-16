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

    #[error("metadata hash mismatch: supplied {supplied} expected {expected}")]
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
    use super::InvalidTransactionMetadata;
    use amaru_kernel::{include_cbor, AuxiliaryData, MintedTransactionBody};
    use test_case::test_case;

    macro_rules! fixture_tx {
        ($title:expr) => {
            include_cbor!(concat!("transactions/preprod/", $title, "/tx.cbor"))
        };
    }

    macro_rules! fixture_aux_data {
        ($title:literal) => {
            include_cbor!(concat!(
                "transactions/preprod/",
                $title,
                "/auxiliary-data.cbor"
            ))
        };
    }

    macro_rules! fixture {
        ($hash:literal) => {
            (fixture_tx!($hash), Some(fixture_aux_data!($hash)))
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_tx!(concat!($hash, "/", $variant)),
                Some(fixture_aux_data!($hash)),
            )
        };
        ($hash:literal, $pat:pat) => {
            (fixture_tx!($hash), None)
        };
    }

    #[test_case(
        fixture!("a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7");
        "valid")
    ]
    #[test_case(
        fixture!("a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7", "missing-adh") =>
        matches Err(InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(hash))
            if &hash.to_string() == "880443667460ae3b3016366d5bf66aca62c8149d67a53e94dde37120adffa624";
        "missing data hash"
    )]
    #[test_case(
        fixture!("a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7", "conflicting-adh") =>
        matches Err(InvalidTransactionMetadata::ConflictingMetadataHash{..});
        "hash mismatch"
    )]
    #[test_case(
        fixture!("a944cb78b60b02a2f50d605717de4c314cbe5fca7cdae6dd58015a3d6dc645d7", None) =>
        matches Err(InvalidTransactionMetadata::MissingTransactionMetadata{..});
        "missing auxiliary data"
    )]
    fn test_metadata(
        (transaction, auxiliary_data): (MintedTransactionBody<'_>, Option<AuxiliaryData>),
    ) -> Result<(), InvalidTransactionMetadata> {
        super::execute(&transaction, auxiliary_data.as_ref())
    }
}
