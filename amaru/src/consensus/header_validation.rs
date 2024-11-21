use crate::{consensus::ValidateHeaderEvent, sync::PullEvent};
use gasket::framework::*;
use miette::miette;
use ouroboros::{ledger::LedgerState, validator::Validator};
use ouroboros_praos::consensus::BlockValidator;
use pallas_crypto::hash::Hash;
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_primitives::conway::Epoch;
use pallas_traverse::MultiEraHeader;
use std::collections::HashMap;
use tracing::{info, trace};

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

pub fn assert_header(
    header: &MultiEraHeader,
    epoch_to_nonce: &HashMap<Epoch, Hash<32>>,
    ledger: &dyn LedgerState,
) -> Result<(), WorkerError> {
    match header {
        MultiEraHeader::BabbageCompatible(_) => {
            let minted_header = header.as_babbage().unwrap();
            let epoch = epoch_for_preprod_slot(minted_header.header_body.slot);

            // TODO: This is awkward, and should probably belong to the LedgerState
            // abstraction? The ledger shall keep track of the rolling nonce and
            // provide some endpoint for the consensus to access it.
            let epoch_nonce = epoch_to_nonce
                .get(&epoch)
                .ok_or(miette!("epoch nonce not found"))
                .or_panic()?;

            // TODO: Take this parameter from an input context, rather than hard-coding it.
            let active_slots_coeff: FixedDecimal =
                FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
            let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
            let block_validator = BlockValidator::new(minted_header, ledger, epoch_nonce, &c);
            block_validator.validate().or_panic()?;
            info!(?minted_header.header_body.block_number, "validated block");
        }
        MultiEraHeader::ShelleyCompatible(_) => {
            trace!("shelley compatible header, skipping validation");
        }
        MultiEraHeader::EpochBoundary(_) => {
            trace!("epoch boundary header, skipping validation");
        }
        MultiEraHeader::Byron(_) => {
            trace!("byron header, skipping validation");
        }
    }

    Ok(())
}

// TODO: Design and implement a proper abstraction for slot arithmetic. See https://github.com/pragma-org/amaru/pull/26/files#r1807394364
/// Mocking this calculation specifically for preprod. We need to know the epoch for the slot so
/// we can look up the epoch nonce.
fn epoch_for_preprod_slot(slot: u64) -> u64 {
    let shelley_epoch_length = 432000; // 5 days in seconds
    let shelley_transition_epoch: u64 = 4;
    let byron_protocol_consts_k: u64 = 2160;
    let byron_epoch_length = 10 * byron_protocol_consts_k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = slot - byron_slots;
    let epoch = (shelley_slots / shelley_epoch_length) + shelley_transition_epoch;

    epoch
}
