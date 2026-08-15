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

use std::sync::Arc;

use amaru_kernel::{ConsensusParameters, EraHistoryError, Header, HeaderHash, IsHeader, Nonce, Point};
use amaru_ouroboros::praos::nonce;
use amaru_ouroboros_traits::{ChainStore, Nonces, Praos, StoreError};
use thiserror::Error;

/// A wrapper around a `ChainStore` that implements the `Praos` trait, supporting nonce evolution.
pub struct PraosChainStore {
    consensus_parameters: Arc<ConsensusParameters>,
    store: Arc<dyn ChainStore>,
}

impl PraosChainStore {
    pub fn new(consensus_parameters: Arc<ConsensusParameters>, store: Arc<dyn ChainStore>) -> Self {
        PraosChainStore { consensus_parameters, store }
    }
}

impl Praos<Header> for PraosChainStore {
    type Error = NoncesError;

    fn get_nonce(&self, header: &HeaderHash) -> Option<Nonce> {
        self.store.get_nonces(header).map(|nonces| nonces.active)
    }

    /// Evolve the given nonce by combining it in an arbitrary way with other data. When
    /// `within_stability_window` is false, this also modifies the candidate nonce for the next
    /// epoch.
    ///
    /// Once the stability window has been reached, the candidate is fixed for the epoch and will
    /// be used once crossing the epoch boundary to produce the next epoch nonce.
    ///
    /// Return the evolved nonces.
    fn evolve_nonce(&self, header: &Header) -> Result<Nonces, Self::Error> {
        let (epoch, is_within_stability_window) = nonce::randomness_stability_window(
            header,
            self.consensus_parameters.era_history(),
            self.consensus_parameters.randomness_stabilization_window(),
        )
        .map_err(NoncesError::EraHistoryError)?;

        // Get the necessary data from the store
        let parent_hash = header.parent().unwrap_or((&Point::Origin).into());
        let parent_nonces = self
            .store
            .get_nonces(&parent_hash)
            .ok_or_else(|| NoncesError::UnknownParent { header: header.hash(), parent: parent_hash })?;

        // The last header of the previous epoch is only needed when crossing an epoch boundary.
        let previous_epoch_tail_parent_hash = if epoch > parent_nonces.epoch {
            self.store
                .load_header(&parent_nonces.tail)
                .ok_or(NoncesError::UnknownHeader { header: parent_nonces.tail })?
                .parent_hash()
        } else {
            None
        };

        // Evolve the nonces per se
        Ok(nonce::evolve_nonces(
            header,
            &parent_nonces,
            epoch,
            is_within_stability_window,
            previous_epoch_tail_parent_hash,
        ))
    }
}

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NoncesError {
    #[error("cannot find nonces: unknown parent {parent} from header {header}")]
    UnknownParent { header: HeaderHash, parent: HeaderHash },

    #[error("unknown header: {header}")]
    UnknownHeader { header: HeaderHash },

    #[error("{0}")]
    StoreError(#[from] StoreError),

    #[error("{0}")]
    EraHistoryError(#[from] EraHistoryError),
}

#[cfg(test)]
mod test {
    use std::sync::{Arc, LazyLock};

    use amaru_kernel::{
        Epoch, EraHistory, GlobalParameters, Header, IsHeader, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS,
        from_cbor, hash, to_cbor,
    };
    use amaru_ouroboros_traits::{WriteChainStore, in_memory_chain_store::InMemoryChainStore};
    use proptest::{prelude::*, prop_compose, proptest};

    use super::*;
    use crate::test::include_header;

    // Epoch 164's last header
    include_header!(PREPROD_HEADER_69638382, 69638382);

    // Epoch 165's before-last header
    include_header!(PREPROD_HEADER_70070331, 70070331);
    static PREPROD_NONCES_70070331: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(165),
        active: hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a"),
        candidate: hash!("74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6",),
        evolving: hash!("9b945f3c45b140f796f0d2ec81c48b50730044bf75eb7208c85f6195f68e9b8c"),
        tail: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7",),
    });

    // Epoch 165's last header
    include_header!(PREPROD_HEADER_70070379, 70070379);
    static PREPROD_NONCES_70070379: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(165),
        active: hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a"),
        candidate: hash!("74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6"),
        evolving: hash!("24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718"),
        tail: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
    });

    // Epoch 166's first header
    include_header!(PREPROD_HEADER_70070426, 70070426);
    static PREPROD_NONCES_70070426: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(166),
        active: hash!("b2853ec951e7ed91b674a47c8276189f414e22b19d61d9da0ac7490801e4bf0d"),
        candidate: hash!("fd6b302f9e0f02cdc784b3d6ca4652788a6e2c5b27f5771509846ee2beb7508c",),
        evolving: hash!("fd6b302f9e0f02cdc784b3d6ca4652788a6e2c5b27f5771509846ee2beb7508c"),
        tail: hash!("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d",),
    });

    // Epoch 166's second header
    include_header!(PREPROD_HEADER_70070464, 70070464);
    static PREPROD_NONCES_70070464: LazyLock<Nonces> = LazyLock::new(|| Nonces {
        epoch: Epoch::from(166),
        active: hash!("b2853ec951e7ed91b674a47c8276189f414e22b19d61d9da0ac7490801e4bf0d"),
        candidate: hash!("18eec9f448f64ebe173563b5bca7d9f788f0db83653a49c449285f4770e9adb1"),
        evolving: hash!("18eec9f448f64ebe173563b5bca7d9f788f0db83653a49c449285f4770e9adb1"),
        tail: hash!("d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d"),
    });

    fn evolve_nonce(
        last_header_last_epoch: &Header,
        parent: (&Header, &Nonces),
        current: &Header,
        era_history: &EraHistory,
        global_parameters: &GlobalParameters,
    ) -> Nonces {
        let store = Arc::new(InMemoryChainStore::default());
        let consensus_parameters = Arc::new(ConsensusParameters::new(global_parameters.clone(), era_history));

        // Have at least the last header of the last epoch available.
        store.store_header(last_header_last_epoch).expect("database failure");

        // Have information about the direct parent.
        store.put_nonces(&parent.0.hash(), parent.1).expect("database failure");

        let praos_store = PraosChainStore::new(consensus_parameters, store.clone());
        praos_store.evolve_nonce(current).expect("evolve nonce failed")
    }

    #[test]
    fn evolve_nonce_inside_stability_window() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070331, &PREPROD_NONCES_70070331),
                &PREPROD_HEADER_70070379,
                &PREPROD_ERA_HISTORY,
                &PREPROD_GLOBAL_PARAMETERS
            ),
            *PREPROD_NONCES_70070379
        )
    }

    #[test]
    fn evolve_nonce_at_epoch_boundary() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070379, &PREPROD_NONCES_70070379),
                &PREPROD_HEADER_70070426,
                &PREPROD_ERA_HISTORY,
                &PREPROD_GLOBAL_PARAMETERS
            ),
            *PREPROD_NONCES_70070426
        )
    }

    #[test]
    fn evolve_nonce_outside_stability_window() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_70070379,
                (&PREPROD_HEADER_70070426, &PREPROD_NONCES_70070426),
                &PREPROD_HEADER_70070464,
                &PREPROD_ERA_HISTORY,
                &PREPROD_GLOBAL_PARAMETERS
            ),
            *PREPROD_NONCES_70070464
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
