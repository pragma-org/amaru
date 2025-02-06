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

use super::header::Header;
use miette::Diagnostic;
use pallas_crypto::hash::Hash;
use std::fmt::Display;
use thiserror::Error;

pub mod rocksdb;

#[derive(Error, Diagnostic, Debug)]
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
    H: Header,
{
    fn load_header(&self, hash: &Hash<32>) -> Option<H>;
    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError>;
}
