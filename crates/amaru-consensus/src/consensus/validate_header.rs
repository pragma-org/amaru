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

use crate::{
    ConsensusError,
    consensus::{ValidationFailed, store_effects::EvolveNonceEffect},
    span::adopt_current_span,
};
use amaru_kernel::{Hash, Header, Nonce, Point, protocol_parameters::GlobalParameters, to_cbor};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::HasStakeDistribution;
use async_trait::async_trait;
use pallas_math::math::FixedDecimal;
use pure_stage::{Effects, StageRef, Stageable};
use std::{fmt, sync::Arc};
use tracing::{Instrument, Level, instrument};

use super::DecodedChainSyncEvent;

#[instrument(
    level = Level::TRACE,
    skip_all,
    fields(
        issuer.key = %header.header_body.issuer_vkey,
    ),
)]
pub fn header_is_valid(
    point: &Point,
    header: &Header,
    raw_header_body: &[u8],
    epoch_nonce: &Nonce,
    ledger: &dyn HasStakeDistribution,
    global_parameters: &GlobalParameters,
) -> Result<(), ConsensusError> {
    let active_slot_coeff: FixedDecimal = FixedDecimal::from(1_u64)
        / FixedDecimal::from(global_parameters.active_slot_coeff_inverse as u64);

    praos::header::assert_all(
        header,
        raw_header_body,
        ledger,
        epoch_nonce,
        &active_slot_coeff,
    )
    .and_then(|assertions| {
        use rayon::prelude::*;
        assertions.into_par_iter().try_for_each(|assert| assert())
    })
    .map_err(|e| ConsensusError::InvalidHeader(point.clone(), e))
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidateHeaderState {
    #[serde(skip, default = "default_ledger")]
    pub ledger: Arc<dyn HasStakeDistribution>,
}

impl PartialEq for ValidateHeaderState {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

fn default_ledger() -> Arc<dyn HasStakeDistribution> {
    struct Fake;
    impl HasStakeDistribution for Fake {
        fn get_pool(
            &self,
            _slot: amaru_kernel::Slot,
            _pool: &amaru_kernel::PoolId,
        ) -> Option<amaru_ouroboros::PoolSummary> {
            unimplemented!()
        }

        fn slot_to_kes_period(&self, _slot: amaru_kernel::Slot) -> u64 {
            unimplemented!()
        }

        fn max_kes_evolutions(&self) -> u64 {
            unimplemented!()
        }

        fn latest_opcert_sequence_number(&self, _pool: &amaru_kernel::PoolId) -> Option<u64> {
            unimplemented!()
        }
    }
    Arc::new(Fake)
}

impl fmt::Debug for ValidateHeaderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateHeader")
            .field("ledger", &"<dyn HasStakeDistribution>")
            .finish()
    }
}

impl ValidateHeaderState {
    pub fn new(ledger: Arc<dyn HasStakeDistribution>) -> Self {
        Self { ledger }
    }
}

#[derive(Clone)]
pub struct ValidateHeader {
    initial_state: ValidateHeaderState,
    global_parameters: GlobalParameters,
    downstream: StageRef<DecodedChainSyncEvent>,
    errors: StageRef<ValidationFailed>,
}

impl ValidateHeader {
    pub fn new<D, E>(
        global_parameters: &GlobalParameters,
        initial_state: ValidateHeaderState,
        downstream: D,
        errors: E,
    ) -> Self
    where
        D: Into<StageRef<DecodedChainSyncEvent>>,
        E: Into<StageRef<ValidationFailed>>,
    {
        Self {
            initial_state,
            global_parameters: global_parameters.clone(),
            downstream: downstream.into(),
            errors: errors.into(),
        }
    }
}

#[async_trait]
impl Stageable<DecodedChainSyncEvent, ValidateHeaderState> for ValidateHeader {
    fn initial_state(&self) -> ValidateHeaderState {
        self.initial_state.clone()
    }

    #[instrument(level = Level::TRACE, skip_all, name = "stage.validate_header")]
    async fn run(
        &self,
        state: ValidateHeaderState,
        msg: DecodedChainSyncEvent,
        eff: Effects<DecodedChainSyncEvent>,
    ) -> ValidateHeaderState {
        adopt_current_span(&msg);

        let (peer, point, header) = match &msg {
            DecodedChainSyncEvent::RollForward {
                peer,
                point,
                header,
                ..
            } => (peer, point, header),
            DecodedChainSyncEvent::Rollback { .. } => {
                eff.send(&self.downstream, msg).await;
                return state;
            }
        };

        let span = tracing::trace_span!(
            "consensus.roll_forward",
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        );

        let send_downstream = async {
            let nonces = match eff.external(EvolveNonceEffect::new(header.clone())).await {
                Ok(nonces) => nonces,
                Err(error) => {
                    tracing::error!(%peer, %error, "evolve nonce failed");
                    eff.send(
                        &self.errors,
                        ValidationFailed::new(peer.clone(), point.clone(), error.into()),
                    )
                    .await;
                    return false;
                }
            };
            let epoch_nonce = &nonces.active;

            if let Err(error) = header_is_valid(
                point,
                header,
                to_cbor(&header.header_body).as_slice(),
                epoch_nonce,
                state.ledger.as_ref(),
                &self.global_parameters,
            ) {
                tracing::info!(%peer, %error, "invalid header");
                eff.send(
                    &self.errors,
                    ValidationFailed::new(peer.clone(), point.clone(), error),
                )
                .await;
                false
            } else {
                true
            }
        }
        .instrument(span)
        .await;

        if send_downstream {
            eff.send(&self.downstream, msg).await;
        }
        state
    }
}
