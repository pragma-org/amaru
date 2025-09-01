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

use super::DecodedChainSyncEvent;
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
use async_trait::async_trait;
use pallas_math::math::FixedDecimal;
use pure_stage::{Effects, Stage, StageRef};
use std::sync::Arc;
use tracing::{Level, instrument};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UpstreamAction {
    KickPeer,
    BanPeer,
    OnlyLogging,
}

#[derive(Clone)]
pub struct ValidateHeader {
    global_parameters: GlobalParameters,
    ledger: Arc<dyn HasStakeDistribution>,
    downstream: StageRef<DecodedChainSyncEvent>,
    errors: StageRef<ValidationFailed>,
}

impl ValidateHeader {
    pub fn new(
        global_parameters: &GlobalParameters,
        ledger: Arc<dyn HasStakeDistribution>,
        downstream: impl AsRef<StageRef<DecodedChainSyncEvent>>,
        errors: impl AsRef<StageRef<ValidationFailed>>,
    ) -> Self {
        Self {
            global_parameters: global_parameters.clone(),
            ledger,
            downstream: downstream.as_ref().clone(),
            errors: errors.as_ref().clone(),
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        fields(point.slot=%point.slot_or_default(),
        point.hash=%Hash::<32>::from(point)),
    )]
    async fn validate(
        &self,
        eff: &Effects<DecodedChainSyncEvent>,
        peer: &Peer,
        point: &Point,
        header: &Header,
    ) -> Result<(), ValidationFailed> {
        let nonces = eff
            .external(EvolveNonceEffect::new(header.clone()))
            .await
            .map_err(|error| {
                tracing::error!(%peer, %error, "evolve nonce failed");
                ValidationFailed::new(peer.clone(), ConsensusError::NoncesError(error))
            })?;

        self.header_is_valid(
            point,
            header,
            to_cbor(&header.header_body).as_slice(),
            &nonces.active,
        )
        .map_err(|error| {
            tracing::info!(%peer, %error, "invalid header");
            ValidationFailed::new(peer.clone(), error)
        })
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        fields(issuer.key = %header.header_body.issuer_vkey)
    )]
    fn header_is_valid(
        &self,
        point: &Point,
        header: &Header,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), ConsensusError> {
        let active_slot_coeff: FixedDecimal = FixedDecimal::from(1_u64)
            / FixedDecimal::from(self.global_parameters.active_slot_coeff_inverse as u64);

        praos::header::assert_all(
            header,
            raw_header_body,
            self.ledger.as_ref(),
            epoch_nonce,
            &active_slot_coeff,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(|e| ConsensusError::InvalidHeader(point.clone(), e))
    }
}

#[async_trait]
impl Stage<DecodedChainSyncEvent, ()> for ValidateHeader {
    fn initial_state(&self) {}

    #[instrument(level = Level::TRACE, skip_all, name = "stage.validate_header")]
    async fn run(
        &self,
        _state: (),
        msg: DecodedChainSyncEvent,
        eff: Effects<DecodedChainSyncEvent>,
    ) {
        adopt_current_span(&msg);
        if let DecodedChainSyncEvent::RollForward {
            ref peer,
            ref point,
            ref header,
            ..
        } = msg
        {
            if let Some(e) = self.validate(&eff, peer, point, header).await.err() {
                eff.send(&self.errors, e).await
            } else {
                eff.send(&self.downstream, msg).await
            }
        } else {
            eff.send(&self.downstream, msg).await
        }
    }
}
