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
    consensus::{
        chain_selection::{self, ChainSelector, Fork},
        store::{ChainStore, NoncesError},
    },
    peer::Peer,
    ConsensusError,
};
use amaru_kernel::{Hash, Header, MintedHeader, Nonce, Point, ACTIVE_SLOT_COEFF_INVERSE};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::{HasStakeDistribution, IsHeader, Praos};
use pallas_codec::minicbor;
use pallas_math::math::FixedDecimal;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{instrument, trace, Level, Span};

use super::fetch::ValidateHeaderEvent;

const EVENT_TARGET: &str = "amaru::consensus";

#[instrument(
    level = Level::TRACE,
    skip_all,
    fields(
        issuer.key = %header.header_body.issuer_vkey,
    ),
)]
pub fn assert_header(
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

pub struct Consensus {
    chain_selector: Arc<Mutex<ChainSelector<Header>>>,
    ledger: Box<dyn HasStakeDistribution>,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
}

impl Consensus {
    pub fn new(
        ledger: Box<dyn HasStakeDistribution>,
        store: Arc<Mutex<dyn ChainStore<Header>>>,
        chain_selector: Arc<Mutex<ChainSelector<Header>>>,
    ) -> Self {
        Self {
            chain_selector,
            ledger,
            store,
        }
    }

    fn forward_block<H: IsHeader>(
        &mut self,
        peer: &Peer,
        header: &H,
        span: &Span,
    ) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated(peer.clone(), header.point().clone(), span.clone())
    }

    fn switch_to_fork(
        &mut self,
        peer: &Peer,
        rollback_point: &Point,
        fork: Vec<Header>,
        span: &Span,
    ) -> Vec<ValidateHeaderEvent> {
        let mut result = vec![ValidateHeaderEvent::Rollback(rollback_point.clone())];

        for header in fork {
            result.push(self.forward_block(peer, &header, span));
        }

        result
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.roll_forward",
        fields(
            point.slot = point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        )
    )]
    pub async fn handle_roll_forward(
        &mut self,
        peer: &Peer,
        point: &Point,
        raw_header: &[u8],
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        let minted_header: MintedHeader<'_> = minicbor::decode(raw_header)
            .map_err(|_| ConsensusError::CannotDecodeHeader(point.clone()))?;

        let raw_body = minted_header.header_body.raw_cbor();

        let header = Header::from(minted_header);

        let header_hash = header.hash();

        // first make sure we store the header
        let mut store = self.store.lock().await;

        store.evolve_nonce(&header)?;

        if let Some(ref epoch_nonce) = store.get_nonce(&header_hash) {
            assert_header(point, &header, raw_body, epoch_nonce, self.ledger.as_ref())?;
        } else {
            return Err(NoncesError::UnknownHeader {
                header: header_hash,
            }
            .into());
        }

        store
            .store_header(&header_hash, &header)
            .map_err(|e| ConsensusError::StoreHeaderFailed(point.clone(), e))?;

        drop(store);

        let result = self
            .chain_selector
            .lock()
            .await
            .select_roll_forward(peer, header);

        let span = Span::current();

        let events = match result {
            chain_selection::ForwardChainSelection::NewTip(hdr) => {
                trace!(target: EVENT_TARGET, hash = %hdr.hash(), "new_tip");
                vec![self.forward_block(peer, &hdr, &span)]
            }
            chain_selection::ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                tip: _,
                fork,
            }) => self.switch_to_fork(&peer, &rollback_point, fork, &span),
            chain_selection::ForwardChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, "no_change");
                vec![]
            }
        };

        Ok(events)
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.roll_backward",
        fields(
            point.slot = rollback.slot_or_default(),
            point.hash = %Hash::<32>::from(rollback),
        )
    )]
    pub async fn handle_roll_back(
        &mut self,
        peer: &Peer,
        rollback: &Point,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        let result = self
            .chain_selector
            .lock()
            .await
            .select_rollback(peer, Hash::from(rollback));

        let span = Span::current();

        //info!("result: {:?}", result);
        match result {
            chain_selection::RollbackChainSelection::RollbackTo(hash) => {
                trace!(target: EVENT_TARGET, %hash, "rollback");
                Ok(vec![ValidateHeaderEvent::Rollback(rollback.clone())])
            }
            chain_selection::RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                fork,
                tip: _,
            }) => Ok(self.switch_to_fork(&peer, &rollback_point, fork, &span)),
        }
    }
}
