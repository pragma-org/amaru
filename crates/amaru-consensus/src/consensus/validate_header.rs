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
use amaru_kernel::{to_cbor, Hash, Header, Nonce, Point, ACTIVE_SLOT_COEFF_INVERSE};
use amaru_ouroboros::{praos, Nonces};
use amaru_ouroboros_traits::{HasStakeDistribution, Praos};
use pallas_math::math::FixedDecimal;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{instrument, Level, Span};

use super::PullEvent;

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
) -> Result<(), ConsensusError> {
    let active_slot_coeff: FixedDecimal =
        FixedDecimal::from(1_u64) / FixedDecimal::from(ACTIVE_SLOT_COEFF_INVERSE as u64);

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

pub struct ValidateHeader {
    ledger: Box<dyn HasStakeDistribution>,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
}

impl ValidateHeader {
    pub fn new(
        ledger: Box<dyn HasStakeDistribution>,
        store: Arc<Mutex<dyn ChainStore<Header>>>,
    ) -> Self {
        Self { ledger, store }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.roll_forward",
        fields(
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        )
    )]
    pub async fn handle_roll_forward(
        &mut self,
        peer: &Peer,
        point: &Point,
        header: &Header,
    ) -> Result<PullEvent, ConsensusError> {
        let Nonces {
            active: ref epoch_nonce,
            ..
        } = self.store.lock().await.evolve_nonce(header)?;

        header_is_valid(
            point,
            header,
            to_cbor(&header.header_body).as_slice(),
            epoch_nonce,
            self.ledger.as_ref(),
        )?;

        Ok(PullEvent::RollForward(
            peer.clone(),
            point.clone(),
            header.clone(),
            Span::current(),
        ))
    }

    pub async fn handle_chain_sync(
        &mut self,
        chain_sync: &PullEvent,
    ) -> Result<PullEvent, ConsensusError> {
        match chain_sync {
            PullEvent::RollForward(peer, point, header, _span) => {
                self.handle_roll_forward(peer, point, header).await
            }
            PullEvent::Rollback(_, _) => Ok(chain_sync.clone()),
        }
    }
}
