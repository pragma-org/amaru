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

use crate::{
    BlockBodyBuilder, Hash, Hasher, Header, Slot,
    cardano::{era_history::EraHistory, raw_block::RawBlock},
    size::BLOCK_BODY,
};

/// The block body as the four CBOR items the header commits to, encoded once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockBodyParts {
    pub(crate) encoded: [Vec<u8>; 4],
    pub(crate) commitment: BlockBodyCommitment,
}

/// Position of a transaction within a block.
/// There can only be a maximum of 65536 transactions in a block, so this is a `u16`.
pub type TransactionIndex = u16;

/// The hash and serialized size committed to by a forged header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockBodyCommitment {
    pub hash: Hash<BLOCK_BODY>,
    pub size: u32,
}

/// An error encountered while assembling a network block from finalized body parts.
#[derive(Debug, thiserror::Error)]
pub enum BlockBodyPartsError {
    #[error("forged header does not commit to the block body")]
    CommitmentMismatch,
    #[error("era for the forged slot is unknown: {0}")]
    Era(#[from] crate::EraHistoryError),
}

impl BlockBodyParts {
    pub(crate) fn new(encoded: [Vec<u8>; 4], commitment: BlockBodyCommitment) -> Self {
        Self { encoded, commitment }
    }

    pub fn commitment(&self) -> BlockBodyCommitment {
        self.commitment
    }

    /// Return the encoded body containing no transactions.
    pub fn empty() -> Self {
        BlockBodyBuilder::default()
            .finalize()
            .unwrap_or_else(|_| unreachable!("default BlockBodyBuilder can always be finalized"))
    }

    /// Assemble a network block only when its header commits to these exact body parts.
    pub fn try_into_block(&self, header: &Header, era_history: &EraHistory) -> Result<RawBlock, BlockBodyPartsError> {
        let commitment = self.commitment();
        if header.body().block_body_hash != commitment.hash || header.body().block_body_size != commitment.size {
            return Err(BlockBodyPartsError::CommitmentMismatch);
        }

        let era_tag = era_history.slot_to_era_tag(Slot::from(header.body().slot))?;
        let header_bytes = amaru_minicbor_extra::to_cbor(header);
        let mut out = Vec::with_capacity(3 + header_bytes.len() + commitment.size as usize);
        out.extend_from_slice(&[0x82, u8::from(era_tag), 0x85]);
        out.extend_from_slice(&header_bytes);
        for part in &self.encoded {
            out.extend_from_slice(part);
        }
        Ok(RawBlock::from(out.into_boxed_slice()))
    }
}

/// Hash the four CBOR body components as committed by a Cardano header.
pub(super) fn hash_body_cbor(components: [&[u8]; 4]) -> Hash<BLOCK_BODY> {
    let mut concat = [0u8; 4 * BLOCK_BODY];
    for (i, component) in components.iter().enumerate() {
        let part = Hasher::<{ 8 * BLOCK_BODY }>::hash(component);
        concat[i * BLOCK_BODY..(i + 1) * BLOCK_BODY].copy_from_slice(part.as_ref());
    }
    Hasher::<{ 8 * BLOCK_BODY }>::hash(&concat)
}
