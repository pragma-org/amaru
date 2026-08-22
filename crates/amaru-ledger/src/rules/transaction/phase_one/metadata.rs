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
use amaru_plutus::arena_pool::ArenaPool;
use amaru_uplc::{arena::Arena, flat::FlatDecodeError};
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
    arena_pool: &ArenaPool,
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

            let arena = arena_pool.acquire();
            validate_auxiliary_data_scripts(data, protocol_version, &arena)?;
            Ok(())
        }
    }
}

fn validate_auxiliary_data_scripts(
    data: &AuxiliaryData,
    protocol_version: ProtocolVersion,
    arena: &Arena,
) -> Result<(), FlatDecodeError> {
    data.plutus_v1_scripts().iter().try_for_each(|s| validate_plutus_script(s, protocol_version, arena))?;
    data.plutus_v2_scripts().iter().try_for_each(|s| validate_plutus_script(s, protocol_version, arena))?;
    data.plutus_v3_scripts().iter().try_for_each(|s| validate_plutus_script(s, protocol_version, arena))?;
    Ok(())
}
