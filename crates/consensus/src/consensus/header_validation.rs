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

use super::header::{ConwayHeader, Header};
use amaru_ledger::kernel::epoch_from_slot;
use amaru_ouroboros::{
    consensus::BlockValidator, traits::HasStakeDistribution, validator::Validator,
};
use gasket::framework::*;
use pallas_crypto::hash::Hash;
use pallas_math::math::FixedDecimal;
use pallas_primitives::conway::Epoch;
use std::collections::HashMap;
use tracing::{instrument, warn, Level};

#[instrument(level = Level::TRACE, skip_all)]
pub fn assert_header<'a>(
    header: &ConwayHeader,
    cbor: &'a [u8],
    epoch_to_nonce: &HashMap<Epoch, Hash<32>>,
    ledger: &dyn HasStakeDistribution,
) -> Result<(), WorkerError> {
    let epoch = epoch_from_slot(header.header_body.slot);

    if let Some(epoch_nonce) = epoch_to_nonce.get(&epoch) {
        // TODO: Take this parameter from an input context, rather than hard-coding it.
        let active_slots_coeff: FixedDecimal =
            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);

        let block_validator =
            BlockValidator::new(header, cbor, ledger, epoch_nonce, &active_slots_coeff);

        block_validator
            .validate()
            .map_err(|e| {
                warn!("fail to validate header {}: {:?}", header.hash(), e);
            })
            .or(Ok(())) // FIXME: Remove this once we have a proper error handling
    } else {
        Ok(())
    }
}
