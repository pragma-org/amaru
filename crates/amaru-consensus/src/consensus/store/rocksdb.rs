use super::{ChainStore, StoreError};
use amaru_kernel::{cbor, from_cbor, to_cbor, Hash};
use amaru_ouroboros_traits::is_header::IsHeader;
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

const NONCES_PREFIX: [u8; 5] = [0x6e, 0x6f, 0x6e, 0x63, 0x65];

impl<H: IsHeader + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.db
            .get_pinned(hash)
            .ok()
            .and_then(|bytes| from_cbor(bytes?.as_ref()))
    }

    #[instrument(level = Level::TRACE, skip_all, fields(%hash))]
    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), super::StoreError> {
        self.db
            .put(hash, to_cbor(header))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn get_nonces(&self, header: &Hash<32>) -> Option<super::Nonces> {
        self.db
            .get_pinned([&NONCES_PREFIX[..], &header[..]].concat())
            .ok()
            .flatten()
            .as_deref()
            .and_then(from_cbor)
    }

    fn put_nonces(
        &mut self,
        header: &Hash<32>,
        nonces: super::Nonces,
    ) -> Result<(), super::StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(&nonces))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::consensus::chain_selection::tests::random_bytes;
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use std::fs::create_dir;

    #[test]
    fn rocksdb_chain_store_can_get_what_it_puts() {
        let tempdir = tempfile::tempdir().unwrap();
        let basedir = tempdir.into_path().join("rocksdb_chain_store");
        create_dir(&basedir).unwrap();
        let mut store = RocksDBStore::new(basedir.clone()).expect("fail to initialise RocksDB");

        let header = FakeHeader {
            block_number: 1,
            slot: 0,
            parent: None,
            body_hash: random_bytes(32).as_slice().into(),
        };

        store.store_header(&header.hash(), &header).unwrap();
        let header2 = store.load_header(&header.hash()).unwrap();
        assert_eq!(header, header2);
    }
}
