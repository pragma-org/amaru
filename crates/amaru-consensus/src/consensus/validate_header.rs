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

use crate::{ConsensusError, consensus::store::ChainStore};
use amaru_kernel::{
    Hash, Header, Nonce, Point, peer::Peer, protocol_parameters::GlobalParameters, to_cbor,
};
use amaru_ouroboros::{Nonces, praos};
use amaru_ouroboros_traits::{HasStakeDistribution, Praos};
use pallas_math::math::FixedDecimal;
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, StageRef, Void};
use std::{fmt, sync::Arc};
use tokio::sync::Mutex;
use tracing::{Level, Span, instrument};

use super::{DecodedChainSyncEvent, store::NoncesError};

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

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct EvolveNonceEffect {
    header: Header,
}

pub type ValidateHeaderResourceStore = Arc<Mutex<dyn ChainStore<Header>>>;
pub type ValidateHeaderResourceParameters = GlobalParameters;

impl EvolveNonceEffect {
    fn new(header: Header) -> Self {
        Self { header }
    }
}

impl ExternalEffect for EvolveNonceEffect {
    #[allow(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let store = resources
                .get::<ValidateHeaderResourceStore>()
                .expect("EvolveNonceEffect requires a chain store")
                .clone();
            let mut store = store.lock().await;
            let global_parameters = resources
                .get::<ValidateHeaderResourceParameters>()
                .expect("EvolveNonceEffect requires global parameters");
            let result = store.evolve_nonce(&self.header, &global_parameters);
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for EvolveNonceEffect {
    type Response = Result<Nonces, NoncesError>;
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
    pub async fn handle_roll_forward<M, S>(
        &mut self,
        eff: &Effects<M, S>,
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

    pub async fn validate_header<M, S>(
        &mut self,
        eff: &Effects<M, S>,
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
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidationFailed {
    pub peer: Peer,
    pub error: String,
    pub action: UpstreamAction,
}

impl ValidationFailed {
    pub fn kick_peer(peer: Peer, error: String) -> Self {
        Self {
            peer,
            error,
            action: UpstreamAction::KickPeer,
        }
    }

    pub fn ban_peer(peer: Peer, error: String) -> Self {
        Self {
            peer,
            error,
            action: UpstreamAction::BanPeer,
        }
    }

    pub fn only_logging(peer: Peer, error: String) -> Self {
        Self {
            peer,
            error,
            action: UpstreamAction::OnlyLogging,
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
    StageRef<DecodedChainSyncEvent, Void>,
    StageRef<ValidationFailed, Void>,
);

pub async fn stage(
    state: State,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent, State>,
) -> State {
    let (mut state, global, downstream, errors) = state;
    let peer = msg.peer();
    let result = match state.validate_header(&eff, msg, &global).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%peer, %error, "invalid header");
            eff.send(
                &errors,
                ValidationFailed::kick_peer(peer, error.to_string()),
            )
            .await;
            return (state, global, downstream, errors);
        }
    };
    eff.send(&downstream, result).await;
    (state, global, downstream, errors)
}
