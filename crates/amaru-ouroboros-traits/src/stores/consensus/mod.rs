// Copyright 2025 PRAGMA
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

pub mod in_memory_consensus_store;

use crate::{IsHeader, Nonces};
use amaru_kernel::{HeaderHash, RawBlock};
use std::fmt::Display;
use std::iter::successors;
use thiserror::Error;

pub trait ReadOnlyChainStore<H>
where
    H: IsHeader,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<H>;
    fn load_headers(&self) -> Box<dyn Iterator<Item = H> + '_>;
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_>;
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_>;
    fn load_parents_children(&self)
    -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_>;
    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;
    fn get_anchor_hash(&self) -> HeaderHash;
    fn get_best_chain_hash(&self) -> HeaderHash;
    fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError>;
    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces>;
    fn has_header(&self, hash: &HeaderHash) -> bool;

    /// Return the hashes of the best chain fragment, starting from the anchor.
    fn retrieve_best_chain(&self) -> Vec<HeaderHash> {
        let anchor = self.get_anchor_hash();
        let mut best_chain = vec![];
        let mut current_hash = self.get_best_chain_hash();
        while let Some(header) = self.load_header(&current_hash) {
            best_chain.push(current_hash);
            if header.hash() != anchor
                && let Some(parent) = header.parent()
            {
                current_hash = parent;
            } else {
                break;
            }
        }
        best_chain.reverse();
        best_chain
    }

    /// Return the ancestors of the header, including the header itself.
    /// Stop at the anchor of the tree.
    fn ancestors<'a>(&'a self, start: H) -> Box<dyn Iterator<Item = H> + 'a>
    where
        H: 'a,
    {
        let anchor = self.get_anchor_hash();
        Box::new(successors(Some(start), move |h| {
            if h.hash() == anchor {
                None
            } else {
                h.parent().and_then(|p| self.load_header(&p))
            }
        }))
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes<'a>(
        &'a self,
        hash: &HeaderHash,
    ) -> Box<dyn Iterator<Item = HeaderHash> + 'a>
    where
        H: 'a,
    {
        if let Some(header) = self.load_header(hash) {
            Box::new(self.ancestors(header).map(|h| h.hash()))
        } else {
            Box::new(vec![*hash].into_iter())
        }
    }
}

impl<H: IsHeader> ReadOnlyChainStore<H> for Box<dyn ChainStore<H>> {
    fn load_header(&self, hash: &HeaderHash) -> Option<H> {
        self.as_ref().load_header(hash)
    }

    fn load_headers(&self) -> Box<dyn Iterator<Item = H> + '_> {
        self.as_ref().load_headers()
    }

    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        self.as_ref().load_nonces()
    }

    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        self.as_ref().load_blocks()
    }

    fn load_parents_children(
        &self,
    ) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
        self.as_ref().load_parents_children()
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        self.as_ref().get_children(hash)
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        self.as_ref().get_anchor_hash()
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        self.as_ref().get_best_chain_hash()
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError> {
        self.as_ref().load_block(hash)
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        self.as_ref().get_nonces(header)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        self.as_ref().has_header(hash)
    }
}

/// A simple chain store interface that can store and retrieve headers indexed by their hash.
pub trait ChainStore<H>: ReadOnlyChainStore<H> + Send + Sync
where
    H: IsHeader,
{
    fn store_header(&self, header: &H) -> Result<(), StoreError>;
    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
    fn update_best_chain(&self, anchor: &HeaderHash, tip: &HeaderHash) -> Result<(), StoreError>;
    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;
    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;
}

#[derive(Error, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum StoreError {
    WriteError { error: String },
    ReadError { error: String },
    OpenError { error: String },
    NotFound { hash: HeaderHash },
    IncompatibleDbVersions { stored: u16, current: u16 },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "WriteError: {}", error),
            StoreError::ReadError { error } => write!(f, "ReadError: {}", error),
            StoreError::OpenError { error } => write!(f, "OpenError: {}", error),
            StoreError::NotFound { hash } => write!(f, "NotFound: {}", hash),
            StoreError::IncompatibleDbVersions { stored, current } => write!(
                f,
                "Incompatible DB Versions: found {}, expected {}",
                stored, current
            ),
        }
    }
}
