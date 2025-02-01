// Copyright 2024 PRAGMA
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

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ValidationError {
    // FIXME: This error is very unspecific and relies on textual representation
    // of underlying errors which could come from VRF, KES, or anything else.
    // It should be replaced with more specific error type.
    #[error("Validation error: {0}")]
    GenericValidationError(String),
    #[error(
        "Invalid VRF key for pool: expected: {key_hash_from_ledger}, was: {key_hash_from_block}"
    )]
    InvalidVrfKeyForPool {
        key_hash_from_ledger: String,
        key_hash_from_block: String,
    },
    #[error("Invalid block hash, expected: {0}, was: {1}")]
    InvalidBlockHash(String, String),
    #[error("Ledger error: {0}")]
    LedgerError(#[from] crate::ledger::Error),
    #[error("VrfVerificationError: {0}")]
    VrfVerificationError(#[from] crate::vrf::VerificationError),
    #[error("InvalidVrfProofHash, expected: {0}, was: {1}")]
    InvalidVrfProofHash(String, String),
    #[error("InvalidVrfLeaderHash, expected: {0}, was: {1}")]
    InvalidVrfLeaderHash(String, String),
    #[error("InvalidOpcertSequenceNumber: {0}")]
    InvalidOpcertSequenceNumber(String),
    #[error("InvalidOpcertSignature")]
    InvalidOpcertSignature,
    #[error("KesVerificationError: {0}")]
    KesVerificationError(String),
    #[error("Operational Certificate KES period ({opcert_kes_period}) is greater than the block slot KES period ({slot_kes_period})!")]
    OpCertKesPeriodTooLarge {
        opcert_kes_period: u64,
        slot_kes_period: u64,
    },
    #[error("InsufficientPoolStake")]
    InsufficientPoolStake,
}

/// Generic trait for validating any type of data. Designed to be used across threads so validations
/// can be done in parallel.
pub trait Validator: Send + Sync {
    fn validate(&self) -> Result<(), ValidationError>;
}
