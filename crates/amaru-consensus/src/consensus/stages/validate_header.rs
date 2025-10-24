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
use crate::consensus::errors::{ConsensusError, ValidationFailed};
use crate::consensus::span::HasSpan;
use crate::consensus::store::PraosChainStore;
use amaru_kernel::IsHeader;
use amaru_kernel::consensus_events::{DecodedChainSyncEvent, ValidateHeaderEvent};
use amaru_kernel::{BlockHeader, Nonce, protocol_parameters::ConsensusParameters, to_cbor};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::can_validate_blocks::{CanValidateHeaders, HeaderValidationError};
use amaru_ouroboros_traits::{ChainStore, HasStakeDistribution, Praos};
use anyhow::anyhow;
use pure_stage::StageRef;
use std::{fmt, sync::Arc};
use tracing::{Level, instrument, span};

pub async fn stage(state: State, msg: DecodedChainSyncEvent, eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.validate_header");
    let _entered = span.enter();

    let (downstream, errors) = state;

    match &msg {
        DecodedChainSyncEvent::RollForward { peer, header, span } => {
            match eff.ledger().validate_header(header).await {
                Ok(_) => {
                    let msg = ValidateHeaderEvent::Validated {
                        peer: peer.clone(),
                        header: header.clone(),
                        span: span.clone(),
                    };
                    eff.base().send(&downstream, msg).await
                }
                Err(error) => {
                    tracing::error!(%peer, %error, "failed to handle roll forward");
                    eff.base()
                        .send(
                            &errors,
                            ValidationFailed::new(
                                peer,
                                ConsensusError::InvalidHeader(header.point(), error),
                            ),
                        )
                        .await
                }
            }
        }
        DecodedChainSyncEvent::Rollback {
            peer,
            rollback_point,
            span,
        } => {
            let msg = ValidateHeaderEvent::Rollback {
                peer: peer.clone(),
                rollback_point: rollback_point.clone(),
                span: span.clone(),
            };
            eff.base().send(&downstream, msg).await;
        }
    };

    (downstream, errors)
}

type State = (StageRef<ValidateHeaderEvent>, StageRef<ValidationFailed>);

pub struct ValidateHeader {
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore<BlockHeader>>,
    ledger: Arc<dyn HasStakeDistribution>,
}

impl fmt::Debug for ValidateHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateHeader")
            .field("store", &"Arc<dyn ChainStore<H>>")
            .field("ledger", &"Arc<dyn HasStakeDistribution>")
            .finish()
    }
}

impl ValidateHeader {
    pub fn new(
        consensus_parameters: Arc<ConsensusParameters>,
        store: Arc<dyn ChainStore<BlockHeader>>,
        ledger: Arc<dyn HasStakeDistribution>,
    ) -> Self {
        Self {
            consensus_parameters,
            store,
            ledger,
        }
    }

    pub fn validate(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        let epoch_nonce = self.evolve_nonce(header)?;
        self.check_header(
            header,
            to_cbor(&header.header_body()).as_slice(),
            &epoch_nonce,
        )?;
        Ok(())
    }

    #[instrument(
        level = Level::TRACE,
        name = "consensus.evolve_nonce",
        skip_all,
        fields(hash = %header.hash()),
    )]
    fn evolve_nonce(&self, header: &BlockHeader) -> Result<Nonce, ConsensusError> {
        let nonces = PraosChainStore::new(self.consensus_parameters.clone(), self.store.clone())
            .evolve_nonce(header)?;
        Ok(nonces.active)
    }

    #[instrument(
        level = Level::TRACE,
        name = "consensus.check_header",
        skip_all,
        fields(issuer.key = %header.header_body().issuer_vkey)
    )]
    fn check_header(
        &self,
        header: &BlockHeader,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), ConsensusError> {
        praos::header::assert_all(
            self.consensus_parameters.clone(),
            header.header(),
            raw_header_body,
            self.ledger.clone(),
            epoch_nonce,
        )
        .and_then(|assertions| {
            use rayon::prelude::*;
            assertions.into_par_iter().try_for_each(|assert| assert())
        })
        .map_err(|e| {
            ConsensusError::InvalidHeader(header.point(), HeaderValidationError::new(anyhow!(e)))
        })
    }
}

impl CanValidateHeaders for ValidateHeader {
    fn validate_header(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        self.validate(header)
            .map_err(|e| HeaderValidationError::new(anyhow!(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus;
    use crate::consensus::effects::MockLedgerOps;
    use crate::consensus::errors::ConsensusError::NoncesError;
    use amaru_kernel::Point;
    use amaru_kernel::is_header::tests::{any_header_with_some_parent, run};
    use amaru_kernel::network::NetworkName;
    use amaru_kernel::{
        HeaderHash, RawBlock,
        protocol_parameters::{GlobalParameters, TESTNET_GLOBAL_PARAMETERS},
    };
    use amaru_ouroboros_traits::Nonces;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, StoreError};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_handle_roll_forward_evolve_nonce_error() {
        let (header, global_parameters, ledger) = create_test_data();
        let failing_store = FailingStore::new().fail_on_evolve_nonce();
        let consensus_parameters = Arc::new(ConsensusParameters::new(
            global_parameters.clone(),
            NetworkName::Preprod.into(),
            Default::default(),
        ));
        let validate_header =
            ValidateHeader::new(consensus_parameters, Arc::new(failing_store), ledger);

        let result = validate_header.validate(&header);

        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            NoncesError(consensus::store::NoncesError::UnknownParent { .. }) => {
                // Expected error
            }
            other => panic!("Expected NoncesError with UnknownParent, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_handle_roll_forward_fake_error() {
        let (header, global_parameters, ledger) = create_test_data();
        let consensus_parameters = Arc::new(ConsensusParameters::new(
            global_parameters.clone(),
            NetworkName::Preprod.into(),
            Default::default(),
        ));
        let failing_store = FailingStore::new();
        let validate_header =
            ValidateHeader::new(consensus_parameters, Arc::new(failing_store), ledger);

        let result = validate_header.validate(&header);

        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            NoncesError(consensus::store::NoncesError::UnknownParent { parent, .. })
                if Some(parent) == header.parent() =>
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
        store: InMemConsensusStore<BlockHeader>,
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

    impl ReadOnlyChainStore<BlockHeader> for FailingStore {
        fn has_header(&self, hash: &HeaderHash) -> bool {
            self.store.has_header(hash)
        }

        fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
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

        fn get_nonces(&self, hash: &HeaderHash) -> Option<Nonces> {
            self.store.get_nonces(hash)
        }

        fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
            self.store.load_from_best_chain(point)
        }
    }

    impl ChainStore<BlockHeader> for FailingStore {
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

        fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError> {
            self.store.store_header(header)
        }

        fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
            self.store.store_block(hash, block)
        }

        fn put_nonces(&self, hash: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
            self.store.put_nonces(hash, nonces)
        }

        fn set_best_chain(&self, point: &Point) -> Result<(), StoreError> {
            todo!()
        }
    }

    // Helper function to create test data
    fn create_test_data() -> (
        BlockHeader,
        &'static GlobalParameters,
        Arc<dyn HasStakeDistribution>,
    ) {
        // Create a minimal valid header using the default constructor
        let header = run(any_header_with_some_parent());
        // Create a simple global parameters for testing
        let global_parameters = &*TESTNET_GLOBAL_PARAMETERS;
        let ledger = Arc::new(MockLedgerOps);
        (header, global_parameters, ledger)
    }
}
