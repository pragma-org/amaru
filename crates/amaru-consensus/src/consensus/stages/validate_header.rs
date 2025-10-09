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

use crate::consensus::effects::{BaseOps, ConsensusOps, LedgerOps};
use crate::consensus::span::HasSpan;
use crate::consensus::store::PraosChainStore;
use crate::consensus::{
    errors::{ConsensusError, ValidationFailed},
    events::DecodedChainSyncEvent,
};
use amaru_kernel::protocol_parameters::ConsensusParameters;
use amaru_kernel::{Hash, Header, Nonce, Point, to_cbor};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::{ChainStore, Praos};
use pure_stage::StageRef;
use std::{fmt, sync::Arc};
use tracing::{Level, instrument, span};

pub async fn stage(state: State, msg: DecodedChainSyncEvent, eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.validate_header");
    let _entered = span.enter();

    let (consensus_parameters, downstream, errors) = state;
    let state = ValidateHeader::new(consensus_parameters.clone(), eff.store(), eff.ledger());

    match &msg {
        DecodedChainSyncEvent::RollForward {
            peer,
            point,
            header,
            ..
        } => match state.handle_roll_forward(point, header).await {
            Ok(_) => eff.base().send(&downstream, msg).await,
            Err(error) => {
                tracing::error!(%peer, %error, "failed to handle roll forward");
                eff.base()
                    .send(&errors, ValidationFailed::new(peer, error))
                    .await
            }
        },
        DecodedChainSyncEvent::Rollback { .. } => {
            eff.base().send(&downstream, msg).await;
        }
        DecodedChainSyncEvent::CaughtUp { .. } => {
            eff.base().send(&downstream, msg).await;
        }
    };

    (consensus_parameters, downstream, errors)
}

type State = (
    Arc<ConsensusParameters>,
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
);

pub struct ValidateHeader {
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore<Header>>,
    ledger: Arc<dyn LedgerOps>,
}

impl fmt::Debug for ValidateHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateHeader")
            .field("store", &"Arc<dyn ChainStore<H>>")
            .field("ledger", &"Arc<dyn LedgerOps>")
            .finish()
    }
}

impl ValidateHeader {
    pub fn new(
        consensus_parameters: Arc<ConsensusParameters>,
        store: Arc<dyn ChainStore<Header>>,
        ledger: Arc<dyn LedgerOps>,
    ) -> Self {
        Self {
            consensus_parameters,
            store,
            ledger,
        }
    }

    #[instrument(
        level = Level::TRACE,
        name = "consensus.roll_forward",
        skip_all,
        fields(
            point = %point,
            slot = %point.slot_or_default(),
            hash = %Hash::<32>::from(point),
        ),
    )]
    pub async fn handle_roll_forward(
        &self,
        point: &Point,
        header: &Header,
    ) -> Result<(), ConsensusError> {
        let nonces = PraosChainStore::new(self.consensus_parameters.clone(), self.store.clone())
            .evolve_nonce(header)?;
        let epoch_nonce = &nonces.active;

        self.header_is_valid(
            point,
            header,
            to_cbor(&header.header_body).as_slice(),
            epoch_nonce,
        )?;
        Ok(())
    }

    #[instrument(level = Level::TRACE, skip_all, fields(issuer.key = %header.header_body.issuer_vkey)
    )]
    pub fn header_is_valid(
        &self,
        point: &Point,
        header: &Header,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), ConsensusError> {
        praos::header::assert_all(
            self.consensus_parameters.clone(),
            header,
            raw_header_body,
            self.ledger.clone(),
            epoch_nonce,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(|e| ConsensusError::InvalidHeader(point.clone(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus;
    use crate::consensus::effects::{HeaderHash, MockLedgerOps};
    use crate::consensus::errors::ConsensusError::NoncesError;
    use crate::consensus::tests::any_header;
    use amaru_kernel::network::NetworkName;
    use amaru_kernel::{
        HEADER_HASH_SIZE, Header, Point, RawBlock,
        protocol_parameters::{GlobalParameters, TESTNET_GLOBAL_PARAMETERS},
    };
    use amaru_ouroboros::Nonces;
    use amaru_ouroboros_traits::fake::tests::run;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, StoreError};
    use pallas_crypto::hash::Hash;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_handle_roll_forward_evolve_nonce_error() {
        let (point, header, global_parameters, ledger) = create_test_data();
        let failing_store = FailingStore::new().fail_on_evolve_nonce();
        let consensus_parameters = Arc::new(ConsensusParameters::new(
            global_parameters.clone(),
            NetworkName::Preprod.into(),
            Default::default(),
        ));
        let validate_header =
            ValidateHeader::new(consensus_parameters, Arc::new(failing_store), ledger);

        let result = validate_header.handle_roll_forward(&point, &header).await;

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
        let (point, header, global_parameters, ledger) = create_test_data();
        let consensus_parameters = Arc::new(ConsensusParameters::new(
            global_parameters.clone(),
            NetworkName::Preprod.into(),
            Default::default(),
        ));
        let failing_store = FailingStore::new();
        let validate_header =
            ValidateHeader::new(consensus_parameters, Arc::new(failing_store), ledger);

        let result = validate_header.handle_roll_forward(&point, &header).await;

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

    // HELPERS

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
    }

    // Helper function to create test data
    fn create_test_data() -> (Point, Header, &'static GlobalParameters, Arc<dyn LedgerOps>) {
        let point = Point::Origin;

        // Create a minimal valid header using the default constructor
        let header = run(any_header());
        // Create a simple global parameters for testing
        let global_parameters = &*TESTNET_GLOBAL_PARAMETERS;
        let ledger = Arc::new(MockLedgerOps);

        (point, header, global_parameters, ledger)
    }
}
