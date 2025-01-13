use thiserror::Error;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("InvalidByteLength: {0}")]
    InvalidByteLength(String),
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
