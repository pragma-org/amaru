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
    fn get(&self, hash: &Hash<32>) -> Option<H>;
    fn put(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError>;
}
