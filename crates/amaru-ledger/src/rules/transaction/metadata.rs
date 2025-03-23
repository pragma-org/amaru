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
    #[error("Missing metadata: auxiliary data hash {0}")]
    MissingTransactionMetadata(Bytes),
    #[error("Missing auxiliary data hash: metadata hash {0}")]
    MissingTransactionAuxiliaryDataHash(Hash<32>),
    #[error("Metadata hash mismatch: supplied {supplied:?} expected {expected:?}")]
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
