use super::header::{ConwayHeader, Header};
use pallas_crypto::hash::Hash;
use std::path::PathBuf;

pub enum StoreError {}

/// A simple chain store interface that can store and retrieve headers indexed by their hash.
pub trait ChainStore<H>
where
    H: Header,
{
    fn get(&self, hash: &Hash<32>) -> Option<H>;
    fn put(&self, hash: &Hash<32>, header: &H) -> Result<(), StoreError>;
}

/// A simple chain store that lookup/store headers as files on disk
pub struct SimpleChainStore {
    pub basedir: PathBuf,
}

impl SimpleChainStore {
    pub fn new(basedir: PathBuf) -> Self {
        SimpleChainStore { basedir }
    }
}

impl ChainStore<ConwayHeader> for SimpleChainStore {
    fn get(&self, hash: &Hash<32>) -> Option<ConwayHeader> {
        let hash_hex = hex::encode(hash);
        let path = self.basedir.join(hash_hex);
        if path.exists() {
            let data = std::fs::read(path).unwrap();
            let header = ConwayHeader::from_cbor(&data).unwrap();
            Some(header)
        } else {
            None
        }
    }

    fn put(&self, _hash: &Hash<32>, _header: &ConwayHeader) -> Result<(), StoreError> {
        todo!()
    }
}
