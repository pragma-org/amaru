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

use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::store::PraosChainStore;
use crate::consensus::{
    errors::{ConsensusError, ValidationFailed},
    events::DecodedChainSyncEvent,
    span::adopt_current_span,
};
use amaru_kernel::{
    Hash, Header, Nonce, Point, peer::Peer, protocol_parameters::GlobalParameters, to_cbor,
};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::{ChainStore, HasStakeDistribution, Praos};
use pallas_math::math::FixedDecimal;
use pure_stage::{BoxFuture, StageRef};
use std::{fmt, sync::Arc};
use tracing::{Instrument, Level, Span, instrument};

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
            point.hash = %Hash::<32>::from(point),
        )
    )]
    pub async fn handle_roll_forward(
        &mut self,
        store: Arc<dyn ChainStore<Header>>,
        peer: &Peer,
        point: &Point,
        header: &Header,
        global_parameters: &GlobalParameters,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        let nonces = PraosChainStore::new(store).evolve_nonce(header, global_parameters)?;
        let epoch_nonce = &nonces.active;

        header_is_valid(
            point,
            header,
            to_cbor(&header.header_body).as_slice(),
            epoch_nonce,
            self.ledger.as_ref(),
            global_parameters,
        )?;

        Ok(DecodedChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: point.clone(),
            header: header.clone(),
            span: Span::current(),
        })
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
pub fn stage(
    state: State,
    msg: DecodedChainSyncEvent,
    eff: impl ConsensusOps + 'static,
) -> BoxFuture<'static, State> {
    let store = eff.store();
    Box::pin(async move {
        adopt_current_span(&msg);
        let (mut state, global, downstream, errors) = state;

        let (peer, point, header) = match &msg {
            DecodedChainSyncEvent::RollForward {
                peer,
                point,
                header,
                ..
            } => (peer, point, header),
            DecodedChainSyncEvent::Rollback { .. } => {
                eff.base().send(&downstream, msg).await;
                return (state, global, downstream, errors);
            }
            DecodedChainSyncEvent::CaughtUp { .. } => {
                eff.base().send(&downstream, msg).await;
                return (state, global, downstream, errors);
            }
        };

        let span = tracing::trace_span!(
            "consensus.roll_forward",
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        );

        async {
            match state
                .handle_roll_forward(store, peer, point, header, &global)
                .await
            {
                Ok(msg) => eff.base().send(&downstream, msg).await,
                Err(error) => {
                    tracing::error!(%peer, %error, "failed to handle roll forward");
                    eff.base()
                        .send(&errors, ValidationFailed::new(peer, error))
                        .await
                }
            }
        }
        .instrument(span)
        .await;

        (state, global, downstream, errors)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus;
    use crate::consensus::effects::HeaderHash;
    use crate::consensus::errors::ConsensusError::NoncesError;
    use amaru_kernel::{
        Bytes, HEADER_HASH_SIZE, Header, HeaderBody, Point, PseudoHeader, RawBlock,
        peer::Peer,
        protocol_parameters::{GlobalParameters, TESTNET_GLOBAL_PARAMETERS},
    };
    use amaru_ouroboros::{Nonces, OperationalCert, VrfCert};
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, StoreError};
    use amaru_slot_arithmetic::EraHistory;
    use pallas_crypto::hash::Hash;
    use std::sync::Arc;

    // Fake store that returns errors for each operation
    struct FailingStore {
        fail_on_evolve_nonce: bool,
        store: InMemConsensusStore<Header>,
    }

    impl FailingStore {
        fn new() -> Self {
            Self {
                fail_on_evolve_nonce: false,
                store: InMemConsensusStore::new(),
            }
        }

        fn fail_on_evolve_nonce(mut self) -> Self {
            self.fail_on_evolve_nonce = true;
            self
        }
    }

    impl ReadOnlyChainStore<Header> for FailingStore {
        fn has_header(&self, hash: &HeaderHash) -> bool {
            self.store.has_header(hash)
        }

        fn load_header(&self, hash: &HeaderHash) -> Option<Header> {
            self.store.load_header(hash)
        }

        fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
            self.store.get_children(hash)
        }

        fn get_anchor_hash(&self) -> HeaderHash {
            self.store.get_anchor_hash()
        }

        fn get_best_chain_hash(&self) -> HeaderHash {
            self.store.get_best_chain_hash()
        }

        fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError> {
            self.store.load_block(hash)
        }

        fn load_headers(&self) -> Box<dyn Iterator<Item = Header> + '_> {
            self.store.load_headers()
        }

        fn load_nonces(&self) -> Box<dyn Iterator<Item = (Hash<32>, Nonces)> + '_> {
            self.store.load_nonces()
        }

        fn load_blocks(&self) -> Box<dyn Iterator<Item = (Hash<32>, RawBlock)> + '_> {
            self.store.load_blocks()
        }

        fn load_parents_children(
            &self,
        ) -> Box<dyn Iterator<Item = (Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)> + '_>
        {
            self.store.load_parents_children()
        }

        fn get_nonces(&self, hash: &Hash<32>) -> Option<Nonces> {
            self.store.get_nonces(hash)
        }
    }

    impl ChainStore<Header> for FailingStore {
        fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
            self.store.set_anchor_hash(hash)
        }

        fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
            self.store.set_best_chain_hash(hash)
        }

        fn update_best_chain(
            &self,
            anchor: &HeaderHash,
            tip: &HeaderHash,
        ) -> Result<(), StoreError> {
            self.store.update_best_chain(anchor, tip)
        }

        fn store_header(&self, header: &Header) -> Result<(), StoreError> {
            self.store.store_header(header)
        }

        fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
            self.store.store_block(hash, block)
        }

        fn remove_header(&self, hash: &Hash<32>) -> Result<(), StoreError> {
            self.store.remove_header(hash)
        }

        fn put_nonces(&self, hash: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
            self.store.put_nonces(hash, nonces)
        }

        fn era_history(&self) -> &EraHistory {
            self.store.era_history()
        }
    }

    // Helper function to create test data
    fn create_test_data() -> (
        Peer,
        Point,
        Header,
        &'static GlobalParameters,
        Arc<dyn HasStakeDistribution>,
    ) {
        let peer = Peer::new("test-peer");
        let point = Point::Origin;

        // Create a minimal valid header using the default constructor
        let header = PseudoHeader {
            header_body: HeaderBody {
                block_number: 0,
                slot: 1,
                prev_hash: None,
                issuer_vkey: Bytes::from(vec![]),
                vrf_vkey: Bytes::from(vec![]),
                vrf_result: VrfCert(Bytes::from(vec![]), Bytes::from(vec![])),
                block_body_size: 0,
                block_body_hash: Hash::<32>::from([0u8; 32]),
                operational_cert: OperationalCert {
                    operational_cert_hot_vkey: Bytes::from(vec![]),
                    operational_cert_sequence_number: 0,
                    operational_cert_kes_period: 0,
                    operational_cert_sigma: Bytes::from(vec![]),
                },
                protocol_version: (1, 2),
            },
            body_signature: Bytes::from(vec![]),
        };

        // Create a simple global parameters for testing
        let global_parameters = &*TESTNET_GLOBAL_PARAMETERS;

        // Create a simple fake ledger
        struct FakeLedger;
        impl HasStakeDistribution for FakeLedger {
            fn get_pool(
                &self,
                _slot: amaru_kernel::Slot,
                _pool: &amaru_kernel::PoolId,
            ) -> Option<amaru_ouroboros::PoolSummary> {
                None
            }

            fn slot_to_kes_period(&self, _slot: amaru_kernel::Slot) -> u64 {
                0
            }

            fn max_kes_evolutions(&self) -> u64 {
                0
            }

            fn latest_opcert_sequence_number(&self, _pool: &amaru_kernel::PoolId) -> Option<u64> {
                None
            }
        }
        let ledger = Arc::new(FakeLedger);

        (peer, point, header, global_parameters, ledger)
    }

    #[tokio::test]
    async fn test_handle_roll_forward_evolve_nonce_error() {
        let (peer, point, header, global_parameters, ledger) = create_test_data();
        let mut validate_header = ValidateHeader::new(ledger);
        let failing_store = FailingStore::new().fail_on_evolve_nonce();

        let result = validate_header
            .handle_roll_forward(
                Arc::new(failing_store),
                &peer,
                &point,
                &header,
                global_parameters,
            )
            .await;

        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            ConsensusError::NoncesError(consensus::store::NoncesError::UnknownParent {
                ..
            }) => {
                // Expected error
            }
            other => panic!("Expected NoncesError with UnknownParent, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_handle_roll_forward_fake_error() {
        let (peer, point, header, global_parameters, ledger) = create_test_data();
        let mut validate_header = ValidateHeader::new(ledger);
        let failing_store = FailingStore::new();

        let result = validate_header
            .handle_roll_forward(
                Arc::new(failing_store),
                &peer,
                &point,
                &header,
                global_parameters,
            )
            .await;

        //        println!("error is: {:?}", result.unwrap_err());
        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            NoncesError(consensus::store::NoncesError::UnknownParent { parent, .. })
                if parent == point.hash() =>
            {
                // Expected error
            }
            other => panic!("Expected NoncesError with UnknownParent, got: {:?}", other),
        }
    }
}
