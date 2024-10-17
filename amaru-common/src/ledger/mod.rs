use crate::pool::PoolId;
use mockall::automock;
use pallas_traverse::update::RationalNumber;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("PoolId not found")]
    PoolIdNotFound,
}

/// The sigma value of a pool. This is a rational number that represents the total value of the
/// delegated stake in the pool over the total value of the active stake in the network. This value
/// is tracked in the ledger state and recorded as a snapshot value at each epoch.
pub type PoolSigma = RationalNumber;

/// The pool info trait provides a lookup mechanism for pool data. This is sourced from the ledger
#[automock]
pub trait PoolInfo: Send + Sync {
    /// Performs a lookup of a pool_id to its sigma value. This usually represents a different set of
    /// sigma snapshot data depending on whether we need to look up the pool_id in the current epoch
    /// or in the future.
    fn sigma(&self, pool_id: &PoolId) -> Result<PoolSigma, Error>;
    
    /// Hashes the vrf vkey of a pool.
    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<[u8; 32], Error>;
}

/// The slot calculator trait provides a mechanism for calculating slot and epoch information based
/// on the state of the ledger.
#[automock]
pub trait SlotCalculator: Send + Sync {
    /// Performs a lookup of the first slot number in the epoch given an absolute slot number.
    /// This requires knowledge of the genesis values and the shelley transition epoch value when
    /// we left the byron era.
    fn first_slot_in_epoch(&self, absolute_slot: u64) -> u64;
    
    /// Performs a lookup of the epoch number given an absolute slot number. This requires knowledge
    /// of the genesis values and the shelley transition epoch value when we left the byron era.
    fn epoch_for_slot(&self, absolute_slot: u64) -> u64;
}
