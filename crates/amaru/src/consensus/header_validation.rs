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

use crate::{consensus::ValidateHeaderEvent, sync::PullEvent};
use amaru_ledger::kernel::epoch_from_slot;
use gasket::framework::*;
use ouroboros::{consensus::BlockValidator, ledger::LedgerState, validator::Validator};
use pallas_crypto::hash::Hash;
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_primitives::conway::Epoch;
use pallas_traverse::MultiEraHeader;
use std::collections::HashMap;
use tracing::warn;

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
                block_validator.validate().or_panic()?;
            }
        }
        MultiEraHeader::ShelleyCompatible(_) => {
            warn!("shelley compatible header, skipping validation");
        }
        MultiEraHeader::EpochBoundary(_) => {
            warn!("epoch boundary header, skipping validation");
        }
        MultiEraHeader::Byron(_) => {
            warn!("byron header, skipping validation");
        }
    }

    Ok(())
}
