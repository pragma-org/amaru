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

use crate::consensus::{
    effects::store_effects::Store,
    errors::{ConsensusError, ValidationFailed},
    events::DecodedChainSyncEvent,
    span::adopt_current_span,
    store::StoreOps,
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
        mut store: impl StoreOps,
        peer: &Peer,
        point: &Point,
        header: &Header,
        global_parameters: &GlobalParameters,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        let nonces = store.evolve_nonce(peer, header).await?;
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
pub async fn stage(
    state: State,
    msg: DecodedChainSyncEvent,
    mut eff: Effects<DecodedChainSyncEvent>,
) -> State {
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

    async {
        match state
            .handle_roll_forward(Store(&mut eff), peer, point, header, &global)
            .await
        {
            Ok(msg) => eff.send(&downstream, msg).await,
            Err(error) => {
                tracing::error!(%peer, %error, "failed to handle roll forward");
                eff.send(&errors, ValidationFailed::new(peer, error)).await
            }
        }
    }
    .instrument(span)
    .await;

    (state, global, downstream, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::errors::ProcessingFailed;
    use crate::consensus::store::NoncesError;
    use amaru_kernel::{
        Bytes, Epoch, Header, HeaderBody, Point, PseudoHeader,
        peer::Peer,
        protocol_parameters::{GlobalParameters, TESTNET_GLOBAL_PARAMETERS},
    };
    use amaru_ouroboros::{Nonces, OperationalCert, VrfCert, praos::header::AssertHeaderError};
    use pallas_crypto::hash::Hash;
    use std::sync::Arc;

    // Fake store that returns errors for each operation
    struct FailingStore {
        fail_on_evolve_nonce: bool,
    }

    impl FailingStore {
        fn new() -> Self {
            Self {
                fail_on_evolve_nonce: false,
            }
        }

        fn fail_on_evolve_nonce(mut self) -> Self {
            self.fail_on_evolve_nonce = true;
            self
        }
    }

    impl StoreOps for FailingStore {
        async fn store_header(
            &mut self,
            _peer: &Peer,
            _header: &Header,
        ) -> Result<(), ProcessingFailed> {
            Ok(())
        }

        async fn store_block(
            &mut self,
            _peer: &Peer,
            _point: &Point,
            _block: &amaru_kernel::RawBlock,
        ) -> Result<(), ProcessingFailed> {
            Ok(())
        }

        async fn evolve_nonce(
            &mut self,
            _peer: &Peer,
            _header: &Header,
        ) -> Result<Nonces, ConsensusError> {
            if self.fail_on_evolve_nonce {
                Err(ConsensusError::NoncesError(NoncesError::UnknownParent {
                    header: Hash::<32>::from([0u8; 32]),
                    parent: Hash::<32>::from([1u8; 32]),
                }))
            } else {
                // Return a dummy nonces for successful case
                Ok(Nonces {
                    epoch: Epoch::from(0),
                    active: Hash::<32>::from([0u8; 32]),
                    evolving: Hash::<32>::from([1u8; 32]),
                    candidate: Hash::<32>::from([2u8; 32]),
                    tail: Hash::<32>::from([3u8; 32]),
                })
            }
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
            .handle_roll_forward(failing_store, &peer, &point, &header, global_parameters)
            .await;

        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            ConsensusError::NoncesError(NoncesError::UnknownParent { .. }) => {
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
            .handle_roll_forward(failing_store, &peer, &point, &header, global_parameters)
            .await;

        #[allow(clippy::wildcard_enum_match_arm)]
        match result.unwrap_err() {
            ConsensusError::InvalidHeader(p, AssertHeaderError::TryFromSliceError)
                if p == point =>
            {
                // Expected error
            }
            other => panic!("Expected NoncesError with UnknownParent, got: {:?}", other),
        }
    }
}
