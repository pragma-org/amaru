use super::header::Header;
use miette::Diagnostic;
use pallas_crypto::hash::Hash;
use std::{collections::HashMap, fmt::Display, path::PathBuf};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
pub enum StoreError {
    WriteError { error: std::io::Error },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "{}", error),
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

#[derive(Clone)]
pub struct InMemoryChainStore<H> {
    headers: HashMap<Hash<32>, H>,
}

impl<H: Header> InMemoryChainStore<H> {
    pub fn new() -> Self {
        InMemoryChainStore {
            headers: std::collections::HashMap::new(),
        }
    }
}

impl<H: Header + Clone + Sync + Send> ChainStore<H> for InMemoryChainStore<H> {
    fn get(&self, hash: &Hash<32>) -> Option<H> {
        self.headers.get(hash).cloned()
    }

    fn put(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        self.headers.insert(*hash, header.clone());
        Ok(())
    }
}

/// A simple chain store that lookup/store headers as files on disk
#[derive(Clone)]
pub struct SimpleChainStore {
    pub basedir: PathBuf,
}

impl SimpleChainStore {
    pub fn new(basedir: PathBuf) -> Self {
        SimpleChainStore { basedir }
    }
}

impl<H: Header> ChainStore<H> for SimpleChainStore {
    fn get(&self, hash: &Hash<32>) -> Option<H> {
        let hash_hex = hex::encode(hash);
        let path = self.basedir.join(hash_hex);
        if path.exists() {
            println!("getting path {:?}", path);
            let data = std::fs::read(path).ok()?;
            let header = H::from_cbor(&data)?;
            Some(header)
        } else {
            None
        }
    }

    fn put(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        let file = self.basedir.join(hex::encode(hash));
        let data = header.to_cbor();
        println!("data: {:?}", data);
        std::fs::write(file, data).map_err(|e| StoreError::WriteError { error: e })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::consensus::header::{random_bytes, TestHeader};
    use std::fs::create_dir;

    #[test]
    fn simple_chain_store_can_get_what_it_puts() {
        let tempdir = tempfile::tempdir().unwrap();
        let basedir = tempdir.into_path().join("simple_chain_store");
        create_dir(&basedir).unwrap();
        let mut store = SimpleChainStore::new(basedir.clone());

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        store.put(&header.hash(), &header).unwrap();
        let header2 = store.get(&header.hash()).unwrap();
        assert_eq!(header, header2);
    }
}
