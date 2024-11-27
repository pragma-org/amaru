use crate::{consensus::ValidateHeaderEvent, ledger::kernel::epoch_from_slot, sync::PullEvent};
use gasket::framework::*;
use ouroboros::{ledger::LedgerState, validator::Validator};
use ouroboros_praos::consensus::BlockValidator;
use pallas_crypto::hash::Hash;
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_primitives::conway::Epoch;
use pallas_traverse::MultiEraHeader;
use std::collections::HashMap;
use tracing::{trace, warn};

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
            let epoch = epoch_from_slot(minted_header.header_body.slot);

            if let Some(epoch_nonce) = epoch_to_nonce.get(&epoch) {
                // TODO: Take this parameter from an input context, rather than hard-coding it.
                let active_slots_coeff: FixedDecimal =
                    FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
                let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
                let block_validator = BlockValidator::new(minted_header, ledger, epoch_nonce, &c);
                block_validator
                    .validate()
                    .unwrap_or_else(|e| warn!(error = ?e, "block validation failed"));
            } else {
                warn!(height = ?minted_header.header_body.block_number, "missing epoch nonce; skipping validation");
            }
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
