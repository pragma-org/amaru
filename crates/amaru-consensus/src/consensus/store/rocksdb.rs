use super::{ChainStore, StoreError};
use crate::consensus::header::Header;
use amaru_kernel::{Epoch, Hash, Nonce};
use rocksdb::{OptimisticTransactionDB, Options};
use std::path::PathBuf;
use tracing::{instrument, Level};

pub struct RocksDBStore {
    pub basedir: PathBuf,
    db: OptimisticTransactionDB,
}

impl RocksDBStore {
    pub fn new(basedir: PathBuf) -> Result<Self, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        Ok(Self {
            db: OptimisticTransactionDB::open(&opts, &basedir).map_err(|e| {
                StoreError::OpenError {
                    error: e.to_string(),
                }
            })?,
            basedir,
        })
    }
}

impl<H: Header> ChainStore<H> for RocksDBStore {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.db
            .get_pinned(hash)
            .ok()
            .and_then(|bytes| H::from_cbor(bytes?.as_ref()))
    }

    #[instrument(level = Level::TRACE, skip_all, fields(%hash))]
    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), super::StoreError> {
        self.db
            .put(hash, header.to_cbor())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn get_nonce(&self, epoch: Epoch) -> Option<Nonce> {
        self.db
            .get_pinned(epoch.to_be_bytes())
            .ok()
            .flatten()
            .map(|bytes| Nonce::try_from(bytes.as_ref()).unwrap_or_else(|e| {
                panic!("Unable to deserialise previously serialized nonce for epoch={epoch}: {e:?}");
            }))
    }

    fn insert_nonce(&mut self, epoch: Epoch, nonce: Nonce) -> Result<(), super::StoreError> {
        self.db
            .put(epoch.to_be_bytes(), &nonce[..])
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::consensus::header::test::{random_bytes, TestHeader};
    use std::fs::create_dir;

    #[test]
    fn rocksdb_chain_store_can_get_what_it_puts() {
        let tempdir = tempfile::tempdir().unwrap();
        let basedir = tempdir.into_path().join("rocksdb_chain_store");
        create_dir(&basedir).unwrap();
        let mut store = RocksDBStore::new(basedir.clone()).expect("fail to initialise RocksDB");

        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
            body_hash: random_bytes(32).as_slice().into(),
        };

        store.store_header(&header.hash(), &header).unwrap();
        let header2 = store.load_header(&header.hash()).unwrap();
        assert_eq!(header, header2);
    }
}
