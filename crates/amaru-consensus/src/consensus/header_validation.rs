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
        header::{point_hash, ConwayHeader, Header},
        store::ChainStore,
    },
    peer::{Peer, PeerSession},
    ConsensusError,
};
use amaru_kernel::{
    epoch_from_slot, Epoch, Hasher, MintedHeader, Nonce, Point, ACTIVE_SLOT_COEFF_INVERSE,
};
use amaru_ledger::ValidateBlockEvent;
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::HasStakeDistribution;
use pallas_codec::minicbor;
use pallas_math::math::FixedDecimal;
use pallas_traverse::ComputeHash;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::{instrument, trace, trace_span, Level, Span};

const EVENT_TARGET: &str = "amaru::consensus";

#[instrument(
    level = Level::TRACE,
    skip_all,
    fields(
        header.hash = %Hasher::<256>::hash(header.header_body.raw_cbor()),
        header.slot = header.header_body.slot,
        issuer.key = %header.header_body.issuer_vkey,
    ),
)]
pub fn assert_header<'a>(
    point: &Point,
    header: &'a MintedHeader<'a>,
    epoch_nonce: &Nonce,
    ledger: &dyn HasStakeDistribution,
) -> Result<(), ConsensusError> {
    let active_slot_coeff: FixedDecimal =
        FixedDecimal::from(1_u64) / FixedDecimal::from(ACTIVE_SLOT_COEFF_INVERSE as u64);

    praos::header::assert_all(header, ledger, epoch_nonce, &active_slot_coeff)
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(|e| ConsensusError::InvalidHeader(point.clone(), e))
}

pub struct Consensus {
    peer_sessions: HashMap<Peer, PeerSession>,
    chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
    ledger: Box<dyn HasStakeDistribution>,
    store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
}

impl Consensus {
    pub fn new(
        peer_sessions: Vec<PeerSession>,
        ledger: Box<dyn HasStakeDistribution>,
        store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
        chain_selector: Arc<Mutex<ChainSelector<ConwayHeader>>>,
    ) -> Self {
        let peer_sessions = peer_sessions
            .into_iter()
            .map(|p| (p.peer.clone(), p))
            .collect::<HashMap<_, _>>();
        Self {
            peer_sessions,
            chain_selector,
            ledger,
            store,
        }
    }

    async fn forward_block(
        &mut self,
        peer: &Peer,
        header: &dyn Header,
        parent_span: &Span,
    ) -> Result<ValidateBlockEvent, ConsensusError> {
        let point = header.point();
        let block = {
            // FIXME: should not crash if the peer is not found
            let peer_session = self
                .peer_sessions
                .get(peer)
                .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?;
            let mut session = peer_session.peer_client.lock().await;
            let client = (*session).blockfetch();
            let new_point: pallas_network::miniprotocols::Point = match point.clone() {
                Point::Origin => pallas_network::miniprotocols::Point::Origin,
                Point::Specific(slot, hash) => {
                    pallas_network::miniprotocols::Point::Specific(slot, hash)
                }
            };
            client
                .fetch_single(new_point.clone())
                .await
                .map_err(|_| ConsensusError::FetchBlockFailed(point.clone()))?
        };

        Ok(ValidateBlockEvent::Validated(
            point,
            block,
            parent_span.clone(),
        ))
    }

    async fn switch_to_fork(
        &mut self,
        peer: &Peer,
        rollback_point: &Point,
        fork: Vec<ConwayHeader>,
        parent_span: &Span,
    ) -> Result<Vec<ValidateBlockEvent>, ConsensusError> {
        let mut result = vec![ValidateBlockEvent::Rollback(rollback_point.clone())];

        for header in fork {
            result.push(self.forward_block(peer, &header, parent_span).await?);
        }

        Ok(result)
    }

    pub async fn handle_roll_forward(
        &mut self,
        peer: &Peer,
        point: &Point,
        raw_header: &[u8],
        parent_span: &Span,
    ) -> Result<Vec<ValidateBlockEvent>, ConsensusError> {
        let span = trace_span!(
          target: EVENT_TARGET,
          parent: parent_span,
          "handle_roll_forward",
          slot = ?point.slot_or_default(),
          hash = point_hash(point).to_string())
        .entered();

        let header: MintedHeader<'_> = minicbor::decode(raw_header)
            .map_err(|_| ConsensusError::CannotDecodeHeader(point.clone()))?;

        // first make sure we store the header
        let mut store = self.store.lock().await;

        // FIXME: move into chain_selector
        let epoch = epoch_from_slot(header.header_body.slot);
        if let Some(ref epoch_nonce) = store.get_nonce(&epoch) {
            assert_header(point, &header, epoch_nonce, self.ledger.as_ref())?;
        } else {
            return Err(ConsensusError::MissingNonceForEpoch(epoch));
        }

        let header: ConwayHeader = ConwayHeader::from(header);

        store
            .store_header(&header.compute_hash(), &header)
            .map_err(|e| ConsensusError::StoreHeaderFailed(point.clone(), e))?;

        drop(store);

        let result = self
            .chain_selector
            .lock()
            .await
            .select_roll_forward(peer, header.clone());

        let events = match result {
            chain_selection::ForwardChainSelection::NewTip(hdr) => {
                trace!(target: EVENT_TARGET, hash = %hdr.hash(), "new_tip");
                vec![self.forward_block(peer, &hdr, parent_span).await?]
            }
            chain_selection::ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                tip: _,
                fork,
            }) => {
                self.switch_to_fork(&peer, &rollback_point, fork, parent_span)
                    .await?
            }
            chain_selection::ForwardChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, hash = %header.hash(), "no_change");
                vec![]
            }
        };

        span.exit();

        Ok(events)
    }

    #[instrument(level = Level::TRACE, skip(self, parent_span))]
    pub async fn handle_roll_back(
        &mut self,
        peer: &Peer,
        rollback: &Point,
        parent_span: &Span,
    ) -> Result<Vec<ValidateBlockEvent>, ConsensusError> {
        let result = self
            .chain_selector
            .lock()
            .await
            .select_rollback(peer, point_hash(rollback));

        match result {
            chain_selection::RollbackChainSelection::RollbackTo(hash) => {
                trace!(target: EVENT_TARGET, %hash, "rollback");
                Ok(vec![ValidateBlockEvent::Rollback(rollback.clone())])
            }
            chain_selection::RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                fork,
                tip: _,
            }) => {
                self.switch_to_fork(&peer, &rollback_point, fork, parent_span)
                    .await
            }
        }
    }
}
