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

use amaru_kernel::{Hash, RawBlock};
use amaru_ouroboros_traits::{IsHeader, Nonces};
use amaru_slot_arithmetic::EraHistory;
use std::fmt::Display;
use thiserror::Error;

#[derive(Error, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
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

pub trait ReadOnlyChainStore<H>
where
    H: IsHeader,
{
    fn load_header(&self, hash: &Hash<32>) -> Option<H>;
    fn get_children(&self, hash: &Hash<32>) -> Vec<Hash<32>>;
    fn get_anchor_hash(&self) -> Hash<32>;
    fn get_best_chain_hash(&self) -> Hash<32>;
    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError>;
    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces>;
    fn has_header(&self, hash: &Hash<32>) -> bool;
}

impl<H: IsHeader> ReadOnlyChainStore<H> for Box<dyn ChainStore<H>> {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.as_ref().load_header(hash)
    }
    fn get_children(&self, hash: &Hash<32>) -> Vec<Hash<32>> {
        self.as_ref().get_children(hash)
    }
    fn get_anchor_hash(&self) -> Hash<32> {
        self.as_ref().get_anchor_hash()
    }
    fn get_best_chain_hash(&self) -> Hash<32> {
        self.as_ref().get_best_chain_hash()
    }
    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        self.as_ref().load_block(hash)
    }

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        self.as_ref().get_nonces(header)
    }

    fn has_header(&self, hash: &Hash<32>) -> bool {
        self.as_ref().has_header(hash)
    }
}

/// A simple chain store interface that can store and retrieve headers indexed by their hash.
pub trait ChainStore<H>: ReadOnlyChainStore<H> + Send + Sync
where
    H: IsHeader,
{
    fn store_header(&self, header: &H) -> Result<(), StoreError>;
    fn set_anchor_hash(&self, hash: &Hash<32>) -> Result<(), StoreError>;
    fn set_best_chain_hash(&self, hash: &Hash<32>) -> Result<(), StoreError>;
    fn remove_header(&self, hash: &Hash<32>) -> Result<(), StoreError>;
    fn store_block(&self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError>;
    fn put_nonces(&self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError>;
    fn era_history(&self) -> &EraHistory;
}
