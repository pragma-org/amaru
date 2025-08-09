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

use crate::{consensus::store::ChainStore, peer::Peer, ConsensusError};
use amaru_kernel::{protocol_parameters::GlobalParameters, to_cbor, Hash, Header, Nonce, Point};
use amaru_ouroboros::{praos, Nonces};
use amaru_ouroboros_traits::{HasStakeDistribution, Praos};
use pallas_math::math::FixedDecimal;
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, StageRef, Void};
use std::{fmt, sync::Arc};
use tokio::sync::Mutex;
use tracing::{instrument, Level, Span};

use super::{store::NoncesError, DecodedChainSyncEvent};

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
            .field("store", &"<dyn ChainStore>")
            .finish()
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct EvolveNonceEffect {
    header: Header,
}

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
                .get::<Arc<Mutex<dyn ChainStore<Header>>>>()
                .expect("EvolveNonceEffect requires a chain store")
                .clone();
            let mut store = store.lock().await;
            let global_parameters = resources
                .get::<GlobalParameters>()
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
    pub async fn handle_roll_forward(
        &mut self,
        eff: &Effects<
            DecodedChainSyncEvent,
            (
                ValidateHeader,
                GlobalParameters,
                StageRef<DecodedChainSyncEvent, Void>,
            ),
        >,
        peer: Peer,
        point: Point,
        header: Header,
        global_parameters: &GlobalParameters,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        let Nonces {
            active: ref epoch_nonce,
            ..
        } = eff.external(EvolveNonceEffect::new(header.clone())).await?;

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

    pub async fn validate_header(
        &mut self,
        eff: &Effects<
            DecodedChainSyncEvent,
            (
                ValidateHeader,
                GlobalParameters,
                StageRef<DecodedChainSyncEvent, Void>,
            ),
        >,
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

type State = (
    ValidateHeader,
    GlobalParameters,
    StageRef<DecodedChainSyncEvent, Void>,
);

pub async fn stage(
    state: State,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent, State>,
) -> Result<State, anyhow::Error> {
    let (mut state, global, downstream) = state;
    let result = state.validate_header(&eff, msg, &global).await?;
    eff.send(&downstream, result).await;
    Ok((state, global, downstream))
}
