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

use amaru_kernel::{AuxiliaryData, Bytes, Hash, ProtocolVersion, TransactionBody};
use amaru_uplc::{arena::Arena, flat::FlatDecodeError, machine::PlutusVersion};
use thiserror::Error;

use crate::rules::transaction::phase_one::scripts::validate_plutus_script;

#[derive(Error, Debug)]
pub enum InvalidTransactionMetadata {
    #[error("missing metadata: auxiliary data hash {0}")]
    MissingTransactionMetadata(Bytes),

    #[error("missing auxiliary data hash: metadata hash {0}")]
    MissingTransactionAuxiliaryDataHash(Hash<{ AuxiliaryData::HASH_SIZE }>),

    #[error("metadata hash mismatch: supplied {supplied} expected {expected}")]
    ConflictingMetadataHash {
        supplied: Hash<{ AuxiliaryData::HASH_SIZE }>,
        expected: Hash<{ AuxiliaryData::HASH_SIZE }>,
    },
    #[error("Invalid script bytes: {0}")]
    InvalidScriptBytes(#[from] FlatDecodeError),
}

pub fn execute(
    transaction: &TransactionBody,
    auxiliary_data: Option<&AuxiliaryData>,
    protocol_version: ProtocolVersion,
) -> Result<(), InvalidTransactionMetadata> {
    match (transaction.auxiliary_data_hash.as_ref(), auxiliary_data.map(|aux| (aux, aux.hash()))) {
        (None, None) => Ok(()),
        (None, Some((_data, hash))) => Err(InvalidTransactionMetadata::MissingTransactionAuxiliaryDataHash(hash)),
        (Some(adh), None) => Err(InvalidTransactionMetadata::MissingTransactionMetadata(adh.clone())),
        (Some(supplied_hash), Some((data, expected))) => {
            let supplied = Hash::from(&supplied_hash[..]);
            if expected != supplied {
                return Err(InvalidTransactionMetadata::ConflictingMetadataHash { supplied, expected });
            }

            // TODO: we should not be allocating a new arena here, instead using a shared pool, such as the one we use for phase 2 validation.
            let mut arena = Arena::new();
            validate_auxiliary_data_scripts(data, protocol_version, &mut arena)?;
            Ok(())
        }
    }
}

fn validate_auxiliary_data_scripts(
    data: &AuxiliaryData,
    protocol_version: ProtocolVersion,
    arena: &mut Arena,
) -> Result<(), FlatDecodeError> {
    data.plutus_v1_scripts()
        .iter()
        .try_for_each(|s| validate_plutus_script(s, PlutusVersion::V1, protocol_version, arena))?;
    data.plutus_v2_scripts()
        .iter()
        .try_for_each(|s| validate_plutus_script(s, PlutusVersion::V2, protocol_version, arena))?;
    data.plutus_v3_scripts()
        .iter()
        .try_for_each(|s| validate_plutus_script(s, PlutusVersion::V3, protocol_version, arena))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{AuxiliaryData, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, TransactionBody, include_cbor};
    use test_case::test_case;

    use super::InvalidTransactionMetadata;

    macro_rules! fixture_tx {
        ($title:expr) => {
            include_cbor!(concat!("transactions/preprod/", $title, "/tx.cbor"))
        };
    }

    macro_rules! fixture_aux_data {
        ($title:literal) => {
            include_cbor!(concat!("transactions/preprod/", $title, "/auxiliary-data.cbor"))
        };
    }

    macro_rules! fixture {
        ($hash:literal) => {
            (fixture_tx!($hash), Some(fixture_aux_data!($hash)))
        };
        ($hash:literal, $variant:literal) => {
            (fixture_tx!(concat!($hash, "/", $variant)), Some(fixture_aux_data!($hash)))
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
        fixture!("a7a5b486773edf68c2874d2fd2737f20385be108d31fb25a0be5246d73b7660a");
        "valid but does't roundtrip"
    )]
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
        (transaction, auxiliary_data): (TransactionBody, Option<AuxiliaryData>),
    ) -> Result<(), InvalidTransactionMetadata> {
        super::execute(&transaction, auxiliary_data.as_ref(), PREPROD_DEFAULT_PROTOCOL_PARAMETERS.protocol_version)
    }
}
