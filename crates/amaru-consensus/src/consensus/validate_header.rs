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
use amaru_kernel::{
    Hash, Header, Nonce, Point, peer::Peer, protocol_parameters::GlobalParameters, to_cbor,
};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::HasStakeDistribution;
use pallas_math::math::FixedDecimal;
use pure_stage::{Effects, StageRef};
use std::{fmt, sync::Arc};
use tracing::{Instrument, Level, Span, instrument};

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
pub struct ValidateHeader {
    #[serde(skip, default = "default_ledger")]
    pub ledger: Arc<dyn HasStakeDistribution>,
}

impl PartialEq for ValidateHeader {
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

impl fmt::Debug for ValidateHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateHeader")
            .field("ledger", &"<dyn HasStakeDistribution>")
            .finish()
    }
}

impl ValidateHeader {
    pub fn new(ledger: Arc<dyn HasStakeDistribution>) -> Self {
        Self { ledger }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.roll_forward",
        fields(
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(&point),
        )
    )]
    pub async fn handle_roll_forward<M>(
        &mut self,
        eff: &mut Effects<M>,
        peer: Peer,
        point: Point,
        header: Header,
        global_parameters: &GlobalParameters,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        let nonces = eff.external(EvolveNonceEffect::new(header.clone())).await?;
        let epoch_nonce = &nonces.active;

        header_is_valid(
            &point,
            &header,
            to_cbor(&header.header_body).as_slice(),
            epoch_nonce,
            self.ledger.as_ref(),
            global_parameters,
        )?;

        Ok(DecodedChainSyncEvent::RollForward {
            peer,
            point,
            header,
            span: Span::current(),
        })
    }

    pub async fn validate_header<M>(
        &mut self,
        eff: &mut Effects<M>,
        chain_sync: DecodedChainSyncEvent,
        global_parameters: &GlobalParameters,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        match chain_sync {
            DecodedChainSyncEvent::RollForward {
                peer,
                point,
                header,
                ..
            } => {
                self.handle_roll_forward(eff, peer, point, header, global_parameters)
                    .await
            }
            DecodedChainSyncEvent::Rollback { .. } => Ok(chain_sync),
            DecodedChainSyncEvent::CaughtUp { .. } => Ok(chain_sync),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UpstreamAction {
    KickPeer,
    BanPeer,
    OnlyLogging,
}

type State = (
    ValidateHeader,
    GlobalParameters,
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.validate_header",
)]
pub async fn stage(
    state: State,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent>,
) -> State {
    adopt_current_span(&msg);
    let (state, global, downstream, errors) = state;

    let (peer, point, header) = match &msg {
        DecodedChainSyncEvent::RollForward {
            peer,
            point,
            header,
            ..
        } => (peer, point, header),
        DecodedChainSyncEvent::Rollback { .. } => {
            eff.send(&downstream, msg).await;
            return (state, global, downstream, errors);
        }
        DecodedChainSyncEvent::CaughtUp { .. } => {
            eff.send(&downstream, msg).await;
            return (state, global, downstream, errors);
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
                eff.send(&errors, ValidationFailed::new(peer.clone(), error.into()))
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
            &global,
        ) {
            tracing::info!(%peer, %error, "invalid header");
            eff.send(&errors, ValidationFailed::new(peer.clone(), error))
                .await;
            false
        } else {
            true
        }
    }
    .instrument(span)
    .await;

    if send_downstream {
        eff.send(&downstream, msg).await;
    }

    (state, global, downstream, errors)
}
