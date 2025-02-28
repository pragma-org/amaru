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

use amaru_kernel::{cbor, Epoch, Nonce};
use amaru_ouroboros::praos::nonce;
use amaru_ouroboros_traits::{IsHeader, Praos};
use pallas_crypto::hash::Hash;
use std::fmt::Display;
use thiserror::Error;

pub mod rocksdb;

#[derive(Error, Debug)]
pub enum StoreError {
    WriteError { error: String },
    OpenError { error: String },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "WriteError: {}", error),
            StoreError::OpenError { error } => write!(f, "OpenError: {}", error),
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

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces>;
    fn put_nonces(&mut self, header: &Hash<32>, nonces: Nonces) -> Result<(), StoreError>;
}

#[derive(Debug)]
pub struct Nonces {
    active: Nonce,
    evolving: Nonce,
    candidate: Nonce,
    previous_epoch_last_header: Hash<32>,
    epoch: Epoch,
}

impl<C> cbor::encode::Encode<C> for Nonces {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_array()?;
        e.encode_with(self.active, ctx)?;
        e.encode_with(self.evolving, ctx)?;
        e.encode_with(self.candidate, ctx)?;
        e.encode_with(self.previous_epoch_last_header, ctx)?;
        e.encode_with(self.epoch, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Nonces {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Nonces {
            active: d.decode_with(ctx)?,
            evolving: d.decode_with(ctx)?,
            candidate: d.decode_with(ctx)?,
            previous_epoch_last_header: d.decode_with(ctx)?,
            epoch: d.decode_with(ctx)?,
        })
    }
}

#[derive(Error, Debug)]
pub enum NoncesError {
    #[error("cannot find nonces: unknown parent {parent} from header {header}")]
    UnknownParent { header: Hash<32>, parent: Hash<32> },

    #[error("unknown header: {header}")]
    UnknownHeader { header: Hash<32> },

    #[error("no parent header for: {header} (where one is clearly expected)")]
    NoParentHeader { header: Hash<32> },

    #[error("{0}")]
    StoreError(#[from] StoreError),
}

impl<H: IsHeader> Praos<H> for dyn ChainStore<H> {
    type Error = NoncesError;

    fn get_nonce(&self, header: &Hash<32>) -> Option<Nonce> {
        self.get_nonces(header).map(|nonces| nonces.active)
    }

    fn evolve_nonce(&mut self, header: &H) -> Result<(), Self::Error> {
        let (epoch, is_within_stability_window) = nonce::randomness_stability_window(header);

        let parent_hash = header.parent().ok_or_else(|| NoncesError::NoParentHeader {
            header: header.hash(),
        })?;

        let parent = self
            .get_nonces(&parent_hash)
            .ok_or_else(|| NoncesError::UnknownParent {
                header: header.hash(),
                parent: parent_hash,
            })?;

        // Compute the new evolving nonce by combining it with the current one and the header's VRF
        // output.
        let evolving = nonce::evolve(header, &parent.evolving);

        self.put_nonces(
            &header.hash(),
            Nonces {
                epoch,
                evolving,

                // On epoch changes, compute the new active nonce by combining:
                //   1. the (now stable) candidate; and
                //   2. the previous epoch's last block's parent header hash.
                //
                // If the epoch hasn't changed, then our active nonce is unchanged.
                active: if epoch > parent.epoch {
                    let previous_epoch_last_header = self
                        .load_header(&parent.previous_epoch_last_header)
                        .ok_or(NoncesError::UnknownHeader {
                            header: parent.previous_epoch_last_header,
                        })?;
                    nonce::from_candidate(&previous_epoch_last_header, &parent.candidate).ok_or(
                        NoncesError::NoParentHeader {
                            header: parent.previous_epoch_last_header,
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
                    parent.candidate
                } else {
                    evolving
                },

                // On epoch changes, the parent header is -- by definition -- the last header of the
                // previous epoch.
                //
                // Otherwise, the previous_epoch_last_header remains unchanged.
                previous_epoch_last_header: if epoch > parent.epoch {
                    parent_hash
                } else {
                    parent.previous_epoch_last_header
                },
            },
        )?;

        Ok(())
    }
}
