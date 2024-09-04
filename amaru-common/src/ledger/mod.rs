use crate::pool::PoolId;
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
pub type Sigma = RationalNumber;

/// Performs a lookup of a pool_id to its sigma value. This usually represents a different set of
/// sigma snapshot data depending on whether we need to look up the pool_id in the current epoch
/// or in the future.
pub trait PoolIdToSigma {
    fn sigma(&self, pool_id: &PoolId) -> Result<Sigma, Error>;
}