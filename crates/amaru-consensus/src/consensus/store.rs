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

use amaru_kernel::{protocol_parameters::GlobalParameters, EraHistory, Nonce, Point, RawBlock};
use amaru_ouroboros::{praos::nonce, Nonces};
use amaru_ouroboros_traits::{IsHeader, Praos};
use pallas_crypto::hash::Hash;
use slot_arithmetic::TimeHorizonError;
use std::fmt::Display;
use thiserror::Error;

#[derive(Error, PartialEq, Debug)]
pub enum StoreError {
    WriteError { error: String },
    ReadError { error: String },
    OpenError { error: String },
    NotFound { hash: Hash<32> },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "WriteError: {}", error),
            StoreError::ReadError { error } => write!(f, "ReadError: {}", error),
            StoreError::OpenError { error } => write!(f, "OpenError: {}", error),
            StoreError::NotFound { hash } => write!(f, "NotFound: {}", hash),
        }
    }
}

/// A simple chain store interface that can store and retrieve headers indexed by their hash.
pub trait ChainStore<H>: Send + Sync
where
    H: IsHeader,
{
    fn load_header(&self, hash: &Hash<32>) -> Option<H>;
    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError>;

    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError>;
    fn store_block(&mut self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError>;

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces>;
    fn put_nonces(&mut self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError>;

    fn era_history(&self) -> &EraHistory;
}

impl<H: IsHeader> ChainStore<H> for Box<dyn ChainStore<H>> {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.as_ref().load_header(hash)
    }

    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        self.as_mut().store_header(hash, header)
    }

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        self.as_ref().get_nonces(header)
    }

    fn put_nonces(&mut self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
        self.as_mut().put_nonces(header, nonces)
    }

    fn era_history(&self) -> &EraHistory {
        self.as_ref().era_history()
    }

    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        self.as_ref().load_block(hash)
    }

    fn store_block(&mut self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
        self.as_mut().store_block(hash, block)
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum NoncesError {
    #[error("cannot find nonces: unknown parent {parent} from header {header}")]
    UnknownParent { header: Hash<32>, parent: Hash<32> },

    #[error("unknown header: {header}")]
    UnknownHeader { header: Hash<32> },

    #[error("no parent header for: {header} (where one is clearly expected)")]
    NoParentHeader { header: Hash<32> },

    #[error("{0}")]
    StoreError(#[from] StoreError),

    #[error("{0}")]
    EraHistoryError(#[from] TimeHorizonError),
}

impl<H: IsHeader> Praos<H> for dyn ChainStore<H> {
    type Error = NoncesError;

    fn get_nonce(&self, header: &Hash<32>) -> Option<Nonce> {
        self.get_nonces(header).map(|nonces| nonces.active)
    }

    fn evolve_nonce(
        &mut self,
        header: &H,
        global_parameters: &GlobalParameters,
    ) -> Result<Nonces, Self::Error> {
        let (epoch, is_within_stability_window) =
            nonce::randomness_stability_window(header, self.era_history(), global_parameters)
                .map_err(NoncesError::EraHistoryError)?;

        let parent_hash = header.parent().unwrap_or((&Point::Origin).into());

        let parent = self
            .get_nonces(&parent_hash)
            .ok_or_else(|| NoncesError::UnknownParent {
                header: header.hash(),
                parent: parent_hash,
            })?;

        // Compute the new evolving nonce by combining it with the current one and the header's VRF
        // output.
        let evolving = nonce::evolve(header, &parent.evolving);

        let nonces = Nonces {
            epoch,
            evolving,

            // On epoch changes, compute the new active nonce by combining:
            //   1. the (now stable) candidate; and
            //   2. the previous epoch's last block's parent header hash.
            //
            // If the epoch hasn't changed, then our active nonce is unchanged.
            active: if epoch > parent.epoch {
                let tail = self
                    .load_header(&parent.tail)
                    .ok_or(NoncesError::UnknownHeader {
                        header: parent.tail,
                    })?;
                nonce::from_candidate(&tail, &parent.candidate).ok_or(
                    NoncesError::NoParentHeader {
                        header: parent.tail,
                    },
                )?
            } else {
                parent.active
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
            candidate: if is_within_stability_window {
                evolving
            } else {
                parent.candidate
            },

            // On epoch changes, the parent header is -- by definition -- the last header of the
            // previous epoch.
            //
            // Otherwise, the tail remains unchanged.
            tail: if epoch > parent.epoch {
                parent_hash
            } else {
                parent.tail
            },
        };

        self.put_nonces(&header.hash(), &nonces)?;

        Ok(nonces)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::include_header;
    use amaru_kernel::{from_cbor, hash, network::NetworkName, to_cbor, Header};
    use amaru_ouroboros_traits::{IsHeader, Praos};
    use proptest::{prelude::*, prop_compose, proptest};
    use slot_arithmetic::Epoch;
    use std::{collections::BTreeMap, sync::LazyLock};

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

    #[derive(Default)]
    struct FakeStore {
        headers: BTreeMap<Hash<32>, Header>,
        nonces: BTreeMap<Hash<32>, Nonces>,
    }

    impl ChainStore<Header> for FakeStore {
        fn load_header(&self, hash: &Hash<32>) -> Option<Header> {
            self.headers.get(hash).cloned()
        }

        fn store_header(&mut self, hash: &Hash<32>, header: &Header) -> Result<(), StoreError> {
            self.headers.insert(*hash, header.clone());
            Ok(())
        }

        fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
            self.nonces.get(header).cloned()
        }

        fn put_nonces(&mut self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
            self.nonces.insert(*header, nonces.clone());
            Ok(())
        }

        fn era_history(&self) -> &EraHistory {
            NetworkName::Preprod.into()
        }

        fn load_block(&self, _hash: &Hash<32>) -> Result<RawBlock, StoreError> {
            unimplemented!()
        }

        fn store_block(&mut self, _hash: &Hash<32>, _block: &RawBlock) -> Result<(), StoreError> {
            unimplemented!()
        }
    }

    fn evolve_nonce(
        last_header_last_epoch: &Header,
        parent: (&Header, &Nonces),
        current: &Header,
        global_parameters: &GlobalParameters,
    ) -> Option<Nonces> {
        let mut store = Box::new(FakeStore::default()) as Box<dyn ChainStore<Header>>;

        // Have at least the last header of the last epoch available.
        store
            .store_header(&last_header_last_epoch.hash(), last_header_last_epoch)
            .expect("database failure");

        // Have information about the direct parent.
        store
            .put_nonces(&parent.0.hash(), parent.1)
            .expect("database failure");

        // Evolve the current nonce so that 'get_nonces' can then return a result.
        store
            .evolve_nonce(current, global_parameters)
            .expect("evolve nonce failed");

        store.get_nonces(&current.hash())
    }

    #[test]
    fn evolve_nonce_inside_stability_window() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070331, &PREPROD_NONCES_70070331),
                &PREPROD_HEADER_70070379,
                NetworkName::Preprod.into()
            )
            .as_ref(),
            Some(&*PREPROD_NONCES_70070379)
        )
    }

    #[test]
    fn evolve_nonce_at_epoch_boundary() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_69638382,
                (&PREPROD_HEADER_70070379, &PREPROD_NONCES_70070379),
                &PREPROD_HEADER_70070426,
                NetworkName::Preprod.into()
            )
            .as_ref(),
            Some(&*PREPROD_NONCES_70070426)
        )
    }

    #[test]
    fn evolve_nonce_outside_stability_window() {
        assert_eq!(
            evolve_nonce(
                &PREPROD_HEADER_70070379,
                (&PREPROD_HEADER_70070426, &PREPROD_NONCES_70070426),
                &PREPROD_HEADER_70070464,
                NetworkName::Preprod.into()
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
                tail: Hash::from(tail),
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
