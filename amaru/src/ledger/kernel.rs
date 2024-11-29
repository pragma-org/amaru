// This module acts as an anti-corruption layer between the amaru codebase and pallas. Said
// differently, it mostly re-exports primitives and types from pallas, in a way that prevent pallas
// imports from spreading across the rest of the ledger sub-component.
//
// It's also the right place to put rather general functions or types that ought to be in pallas.
// While elements are being contributed upstream, they might transiently live in this module.

pub use ouroboros::ledger::PoolSigma;
pub use pallas_codec::{
    minicbor as cbor,
    utils::{Nullable, Set},
};
pub use pallas_crypto::hash::{Hash, Hasher};
pub use pallas_primitives::conway::{
    AddrKeyhash, Certificate, Coin, Epoch, MintedBlock, PoolMetadata, RationalNumber, Relay,
    RewardAccount, TransactionInput, TransactionOutput, UnitInterval, VrfKeyhash,
};

// Constants
// ----------------------------------------------------------------------------

/// The maximum depth of a rollback, also known as the security parameter 'k'.
/// This translates down to the length of our volatile storage, containing states of the ledger
/// which aren't yet considered final.
///
// FIXME: import from genesis configuration
pub const CONSENSUS_SECURITY_PARAM: usize = 2160;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Shelley's epoch length.
pub const SHELLEY_EPOCH_LENGTH_SCALE_FACTOR: usize = 200;

/// Number of blocks in a Shelley epoch
pub const SHELLEY_EPOCH_LENGTH: usize =
    SHELLEY_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Byron's epoch length.
pub const BYRON_EPOCH_LENGTH_SCALE_FACTOR: usize = 10;

/// Number of blocks in a Byron epoch
pub const BYRON_EPOCH_LENGTH: usize = BYRON_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Number of slots in the Byron era, for PreProd
pub const BYRON_TOTAL_SLOTS: usize = BYRON_EPOCH_LENGTH * PREPROD_SHELLEY_TRANSITION_EPOCH;

/// Epoch number in which the PreProd network transitioned to Shelley.
pub const PREPROD_SHELLEY_TRANSITION_EPOCH: usize = 4;

// Re-exports
// ----------------------------------------------------------------------------

pub type Point = pallas_network::miniprotocols::Point;

pub type PoolId = Hash<28>;

// PoolParams
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: VrfKeyhash,
    pub pledge: Coin,
    pub cost: Coin,
    pub margin: UnitInterval,
    pub reward_account: RewardAccount,
    pub owners: Set<AddrKeyhash>,
    pub relays: Vec<Relay>,
    pub metadata: Nullable<PoolMetadata>,
}

impl<C> cbor::encode::Encode<C> for PoolParams {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(9)?;
        e.encode_with(self.id, ctx)?;
        e.encode_with(self.vrf, ctx)?;
        e.encode_with(self.pledge, ctx)?;
        e.encode_with(self.cost, ctx)?;
        e.encode_with(&self.margin, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(&self.owners, ctx)?;
        e.encode_with(&self.relays, ctx)?;
        e.encode_with(&self.metadata, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(PoolParams {
            id: d.decode_with(ctx)?,
            vrf: d.decode_with(ctx)?,
            pledge: d.decode_with(ctx)?,
            cost: d.decode_with(ctx)?,
            margin: d.decode_with(ctx)?,
            reward_account: d.decode_with(ctx)?,
            owners: d.decode_with(ctx)?,
            relays: d.decode_with(ctx)?,
            metadata: d.decode_with(ctx)?,
        })
    }
}

// Helpers
// ----------------------------------------------------------------------------

/// Get a 'Point' correspondin to a particular block
pub fn block_point(block: &MintedBlock<'_>) -> Point {
    Point::Specific(
        block.header.header_body.slot,
        Hasher::<256>::hash(block.header.raw_cbor()).to_vec(),
    )
}

/// Calculate the epoch number corresponding to a given slot on the PreProd network.
// FIXME: Design and implement a proper abstraction for slot arithmetic. See https://github.com/pragma-org/amaru/pull/26/files#r1807394364
pub fn epoch_from_slot(slot: u64) -> u64 {
    let shelley_slots = slot - BYRON_TOTAL_SLOTS as u64;
    (shelley_slots / SHELLEY_EPOCH_LENGTH as u64) + PREPROD_SHELLEY_TRANSITION_EPOCH as u64
}

/// Obtain the slot number relative to the epoch.
// FIXME: Design and implement a proper abstraction for slot arithmetic. See https://github.com/pragma-org/amaru/pull/26/files#r1807394364
pub fn relative_slot(slot: u64) -> u64 {
    let shelley_previous_slots = (epoch_from_slot(slot) - PREPROD_SHELLEY_TRANSITION_EPOCH as u64)
        * SHELLEY_EPOCH_LENGTH as u64;
    slot - shelley_previous_slots - BYRON_TOTAL_SLOTS as u64
}
