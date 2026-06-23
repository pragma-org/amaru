// Copyright 2026 PRAGMA
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

use std::sync::{Arc, Mutex};

use amaru_kernel::{
    to_cbor, BlockHeader, ConsensusParameters, Epoch, EraHistory, EraHistoryError, IsHeader, Nonce, Point, PoolId, Slot,
};
use amaru_observability::trace_span;
use amaru_ouroboros_traits::{
    CanValidateHeaders, ChainStore, GetPoolError, HasPools, HeaderValidationError, Nonces, NoncesError, PoolSummary,
    StoreError::ReadError,
};
use anyhow::anyhow;
use num::CheckedSub;

use crate::{
    header_validator::assertions::assert_all,
    state::{StakeDistributions, State},
    store::{HistoricalStores, Store, StoreError},
};

#[derive(Clone)]
pub struct HeaderValidator {
    stake_distributions: StakeDistributions,
    era_history: Arc<EraHistory>,
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore>,
}

impl HeaderValidator {
    #[expect(clippy::unwrap_used)]
    pub fn new<S: Store, HS: HistoricalStores>(
        state: Arc<Mutex<State<S, HS>>>,
        consensus_parameters: Arc<ConsensusParameters>,
        store: Arc<dyn ChainStore>,
    ) -> anyhow::Result<Self> {
        let state = state.lock().unwrap();
        Ok(Self {
            stake_distributions: state.stake_distributions(),
            era_history: Arc::new(state.era_history().clone()),
            consensus_parameters,
            store,
        })
    }

    pub fn validate(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        let epoch_nonce = evolve_nonce(&self.consensus_parameters, self.store.as_ref(), header)
            .map_err(|e| HeaderValidationError::new(anyhow!(e)))?
            .active;
        self.check_header(header, to_cbor(&header.header_body()).as_slice(), &epoch_nonce)?;
        Ok(())
    }

    fn check_header(
        &self,
        header: &BlockHeader,
        raw_header_body: &[u8],
        epoch_nonce: &Nonce,
    ) -> Result<(), HeaderValidationError> {
        let _span = trace_span!(
            amaru_observability::amaru::consensus::validate_header::VALIDATE,
            issuer_key = &header.header_body().issuer_vkey
        );
        let _guard = _span.enter();
        assert_all(self, self.consensus_parameters.clone(), header.header(), raw_header_body, epoch_nonce)
            .and_then(|assertions| {
                use rayon::prelude::*;
                assertions.into_par_iter().try_for_each(|assert| assert())
            })
            .map_err(|e| HeaderValidationError::new(anyhow!(e)))
    }

    #[expect(clippy::unwrap_used)]
    pub fn operational_cert_sequence_number(&self, pool_id: &PoolId) -> Result<Option<u64>, StoreError> {
        // Iterate from most recent to least recent
        for fragment in self.volatile.iter().rev() {
            if fragment.issuer() == *pool_id {
                return Ok(Some(fragment.operational_cert_sequence_number()));
            }
        }
        Ok(self
            .stable
            .lock()
            .unwrap()
            .operational_cert_sequence_number(pool_id)?
            .map(|row| row.latest_opcert_sequence_number))
    }
}

/// Evolve the given nonce by combining it in an arbitrary way with other data. When
/// `within_stability_window` is false, this also modifies the candidate nonce for the next
/// epoch.
///
/// Once the stability window has been reached, the candidate is fixed for the epoch and will
/// be used once crossing the epoch boundary to produce the next epoch nonce.
fn evolve_nonce(
    consensus_parameters: &ConsensusParameters,
    store: &dyn ChainStore,
    header: &BlockHeader,
) -> Result<Nonces, NoncesError> {
    let _span = trace_span!(amaru_observability::amaru::consensus::validate_header::EVOLVE_NONCE, hash = header.hash());
    let _guard = _span.enter();
    let (epoch, is_within_stability_window) =
        consensus_parameters.randomness_stability_window(header.slot()).map_err(NoncesError::EraHistoryError)?;

    let parent_hash = header.parent().unwrap_or((&Point::Origin).into());

    let parent_nonces = store
        .get_nonces(&parent_hash)
        .ok_or_else(|| NoncesError::UnknownParent { header: header.hash(), parent: parent_hash })?;

    // Compute the new evolving nonce by combining it with the current one and the header's VRF
    // output.
    let evolving = parent_nonces.evolving.evolve(header);

    let nonces = Nonces {
        epoch,
        evolving,

        // On epoch changes, compute the new active nonce by combining:
        //   1. the (now stable) candidate; and
        //   2. the previous epoch's last block's parent header hash.
        //
        // If the epoch hasn't changed, then our active nonce is unchanged.
        active: if epoch > parent_nonces.epoch {
            let tail = store
                .load_header(&parent_nonces.tail)
                .ok_or(NoncesError::UnknownHeader { header: parent_nonces.tail })?;
            parent_nonces
                .candidate
                .make_epoch_nonce(&tail)
                .ok_or(NoncesError::NoParentHeader { header: parent_nonces.tail })?
        } else {
            parent_nonces.active
        },

        // Unless we are within the randomness stability window, we also update the candidate. This
        // means that outside of the stability window, we always have:
        //
        //   evolving == candidate
        //
        // They only diverge for the last blocks of each epoch; The candidate remains stable while
        // the rolling nonce keeps evolving in preparation of the next epoch. Another way to look
        // at it is to think that there's always an entire epoch length contributing to the nonce
        // randomness, but it spans over two epochs.
        candidate: if is_within_stability_window { evolving } else { parent_nonces.candidate },

        // On epoch changes, the parent header is -- by definition -- the last header of the
        // previous epoch.
        //
        // Otherwise, the tail remains unchanged.
        tail: if epoch > parent_nonces.epoch { parent_hash } else { parent_nonces.tail },
    };

    store.put_nonces(&header.hash(), &nonces)?;
    Ok(nonces)
}

impl HasPools for HeaderValidator {
    fn get_pool_summary(&self, slot: Slot, pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        let epoch = self
            .era_history
            // NOTE: This function is called by the consensus when validating block headers. So in
            // theory, the slot is either within the current epoch or the next since blocks must
            // form a chain. Either the previous block is well within the current epoch, or it was
            // the last block of the previous epoch.
            //
            // Either way, we do know at this point how to forecast this slot.
            .slot_to_epoch_unchecked_horizon(slot)
            .map_err(GetPoolError::SlotToEpochConversionFailure)?
            .checked_sub(Epoch::TWO)
            .ok_or(GetPoolError::SlotToEpochConversionFailure(EraHistoryError::InvalidEraHistory))?;

        let stake_distributions = self.stake_distributions.0.lock().unwrap();
        let stake_distribution = stake_distributions
            .iter()
            .find(|s| s.epoch == epoch)
            .ok_or(GetPoolError::StakeDistributionNotAvailable(slot, epoch))?;

        match stake_distribution.pools.get(pool) {
            Some(st) => {
                let operational_cert_sequence_number = self
                    .operational_cert_sequence_number(pool)
                    .map_err(|e| GetPoolError::StoreError(ReadError { error: e.to_string() }))?
                    .unwrap_or_default();
                Ok(Some(PoolSummary::new(
                    st.parameters.vrf,
                    st.stake,
                    stake_distribution.active_stake,
                    operational_cert_sequence_number,
                )))
            }
            None => Ok(None),
        }
    }
}

impl CanValidateHeaders for HeaderValidator {
    fn validate_header(&self, header: &BlockHeader) -> Result<(), HeaderValidationError> {
        self.validate(header)
    }
}

#[cfg(test)]
mod test {
    use std::sync::{Arc, LazyLock};

    use amaru_kernel::{
        from_cbor, hash, to_cbor, BlockHeader, Epoch, GlobalParameters, HeaderHash, IsHeader, NetworkName,
    };
    use amaru_ouroboros_traits::{in_memory_chain_store::InMemoryChainStore, BaseReadChainStore, WriteChainStore};
    use proptest::{prelude::*, prop_compose, proptest};

    use super::*;

    macro_rules! include_header {
        ($name:ident, $slot:expr) => {
            static $name: std::sync::LazyLock<BlockHeader> = std::sync::LazyLock::new(|| {
                let data = include_bytes!(concat!("../../tests/data/headers/preprod_", $slot, ".cbor"));
                amaru_kernel::from_cbor(data.as_slice()).expect("invalid header")
            });
        };
    }

    // Epoch 164's last header
    include_header!(PREPROD_HEADER_69638382, 69638382);

    // Epoch 165's before-last header
    include_header!(PREPROD_HEADER_70070331, 70070331);
    static PREPROD_NONCES_70070331: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(165),
        active: hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a").into(),
        candidate: hash!("74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6",).into(),
        evolving: hash!("9b945f3c45b140f796f0d2ec81c48b50730044bf75eb7208c85f6195f68e9b8c").into(),
        tail: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
    });

    // Epoch 165's last header
    include_header!(PREPROD_HEADER_70070379, 70070379);
    static PREPROD_NONCES_70070379: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(165),
        active: hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a").into(),
        candidate: hash!("74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6").into(),
        evolving: hash!("24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718").into(),
        tail: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
    });

    // Epoch 166's first header
    include_header!(PREPROD_HEADER_70070426, 70070426);
    static PREPROD_NONCES_70070426: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(166),
        active: hash!("b2853ec951e7ed91b674a47c8276189f414e22b19d61d9da0ac7490801e4bf0d").into(),
        candidate: hash!("fd6b302f9e0f02cdc784b3d6ca4652788a6e2c5b27f5771509846ee2beb7508c",).into(),
        evolving: hash!("fd6b302f9e0f02cdc784b3d6ca4652788a6e2c5b27f5771509846ee2beb7508c").into(),
        tail: hash!("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d"),
    });

    // Epoch 166's second header
    include_header!(PREPROD_HEADER_70070464, 70070464);
    static PREPROD_NONCES_70070464: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(166),
        active: hash!("b2853ec951e7ed91b674a47c8276189f414e22b19d61d9da0ac7490801e4bf0d").into(),
        candidate: hash!("18eec9f448f64ebe173563b5bca7d9f788f0db83653a49c449285f4770e9adb1").into(),
        evolving: hash!("18eec9f448f64ebe173563b5bca7d9f788f0db83653a49c449285f4770e9adb1").into(),
        tail: hash!("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d"),
    });

    fn call_evolve_nonce(
        last_header_last_epoch: &BlockHeader,
        parent: (&BlockHeader, &Nonces),
        current: &BlockHeader,
        global_parameters: &GlobalParameters,
    ) -> Option<Nonces> {
        let store = Arc::new(InMemoryChainStore::default());
        let consensus_parameters = Arc::new(ConsensusParameters::new(
            global_parameters.clone(),
            NetworkName::Preprod.as_era_history().expect("missing default EraHistory for preprod"),
            Default::default(),
        ));

        // Have at least the last header of the last epoch available.
        store.store_header(last_header_last_epoch).expect("database failure");

        // Have information about the direct parent.
        store.put_nonces(&parent.0.hash(), parent.1).expect("database failure");

        // Evolve the current nonce so that 'get_nonces' can then return a result.
        evolve_nonce(&consensus_parameters, store.as_ref(), current).expect("evolve nonce failed");
        store.get_nonces(&current.hash())
    }

    #[test]
    fn evolve_nonce_inside_stability_window() {
        assert_eq!(
            call_evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070331, &PREPROD_NONCES_70070331),
                &PREPROD_HEADER_70070379,
                NetworkName::Preprod.as_global_parameters().expect("missing default GlobalParameters for preprod")
            )
            .as_ref(),
            Some(&*PREPROD_NONCES_70070379)
        )
    }

    #[test]
    fn evolve_nonce_at_epoch_boundary() {
        assert_eq!(
            call_evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070379, &PREPROD_NONCES_70070379),
                &PREPROD_HEADER_70070426,
                NetworkName::Preprod.as_global_parameters().expect("missing default GlobalParameters for preprod")
            )
            .as_ref(),
            Some(&*PREPROD_NONCES_70070426)
        )
    }

    #[test]
    fn evolve_nonce_outside_stability_window() {
        assert_eq!(
            call_evolve_nonce(
                &PREPROD_HEADER_70070379,
                (&PREPROD_HEADER_70070426, &PREPROD_NONCES_70070426),
                &PREPROD_HEADER_70070464,
                NetworkName::Preprod.as_global_parameters().expect("missing default GlobalParameters for preprod")
            )
            .as_ref(),
            Some(&*PREPROD_NONCES_70070464)
        )
    }

    prop_compose! {
        fn any_nonces()(
            active in any::<[u8; 32]>(),
            evolving in any::<[u8; 32]>(),
            candidate in any::<[u8; 32]>(),
            tail in any::<[u8; 32]>(),
            epoch in any::<Epoch>(),
        ) -> Nonces {
            Nonces {
                active: Nonce::from(active),
                evolving: Nonce::from(evolving),
                candidate: Nonce::from(candidate),
                tail: <HeaderHash>::from(tail),
                epoch,
            }
        }
    }

    proptest! {
        #[test]
        fn prop_nonces_roundtrip_cbor(nonces in any_nonces()) {
            let bytes = to_cbor(&nonces);
            assert_eq!(Some(nonces), from_cbor::<Nonces>(&bytes))
        }
    }
}
