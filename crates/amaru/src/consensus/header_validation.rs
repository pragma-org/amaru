use crate::{consensus::ValidateHeaderEvent, ledger::kernel::epoch_from_slot, sync::PullEvent};
use gasket::framework::*;
use ouroboros::consensus::BlockValidator;
use ouroboros::{ledger::LedgerState, validator::Validator};
use pallas_crypto::hash::Hash;
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_primitives::conway::Epoch;
use std::collections::HashMap;
use tracing::{instrument, warn};

use super::header::ConwayHeader;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

#[instrument(skip_all)]
pub fn assert_header<'a>(
    header: &ConwayHeader,
    cbor: &'a [u8],
    epoch_to_nonce: &HashMap<Epoch, Hash<32>>,
    ledger: &dyn LedgerState,
) -> Result<(), WorkerError> {
    let epoch = epoch_from_slot(header.header_body.slot);

    if let Some(epoch_nonce) = epoch_to_nonce.get(&epoch) {
        // TODO: Take this parameter from an input context, rather than hard-coding it.
        let active_slots_coeff: FixedDecimal =
            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
        let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
        let block_validator = BlockValidator::new(header, cbor, ledger, epoch_nonce, &c);
        block_validator
            .validate()
            .unwrap_or_else(|e| warn!(error = ?e, "block validation failed"));
    }

    Ok(())
}
