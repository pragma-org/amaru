pub(crate) mod common;

use crate::{
    iter::{borrow as iter_borrow, borrow::IterBorrow},
    ledger::{
        kernel::{Epoch, Point, PoolId},
        store::{columns::*, Columns, Store},
    },
};
use ::rocksdb::{self, checkpoint, OptimisticTransactionDB, Options, SliceTransform};
use common::{as_value, PREFIX_LEN};
use miette::Diagnostic;
use pallas_codec::minicbor::{self as cbor};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};
use tracing::{debug, info, warn};

const EVENT_TARGET: &str = "amaru::ledger::store";

/// Special key where we store the tip of the database (most recently applied delta)
const KEY_TIP: &str = "tip";

/// Name of the directory containing the live ledger stable database.
const DIR_LIVE_DB: &str = "live";

/// An opaque handle for a store implementation of top of RocksDB. The database has the
/// following structure:
///
/// * ========================*=============================================== *
/// * key                     * value                                          *
/// * ========================*=============================================== *
/// * 'tip'                   * Point                                          *
/// * 'utxo:'TransactionInput * TransactionOutput                              *
/// * 'pool:'PoolId           * (PoolParams, Vec<(Option<PoolParams>, Epoch)>) *
/// * 'acct:'StakeCredential  * (Option<PoolId>, Lovelace, Lovelace)           *
/// * 'slot':slot             * PoolId
/// * ========================*=============================================== *
///
/// CBOR is used to serialize objects (as keys or values) into their binary equivalent.
pub struct RocksDB {
    /// The working directory where we store the various key/value stores.
    dir: PathBuf,

    /// An instance of RocksDB.
    db: OptimisticTransactionDB,

    /// An ordered (asc) list of epochs for which we have available snapshots
    snapshots: Vec<Epoch>,
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum OpenError {
    #[error(transparent)]
    RocksDB(rocksdb::Error),
    #[error(transparent)]
    IO(io::Error),
    #[error("no ledger stable snapshot found in ledger.db; at least one is expected")]
    NoStableSnapshot,
}

impl RocksDB {
    pub fn new(dir: &Path) -> Result<RocksDB, OpenError> {
        let mut snapshots: Vec<Epoch> = Vec::new();
        for entry in fs::read_dir(dir).map_err(OpenError::IO)?.by_ref() {
            let entry = entry.map_err(OpenError::IO)?;

            if let Ok(epoch) = entry
                .file_name()
                .to_str()
                .unwrap_or_default()
                .parse::<Epoch>()
            {
                debug!(target: EVENT_TARGET, epoch, "new.found_snapshot");
                snapshots.push(epoch);
            } else if entry.file_name() != DIR_LIVE_DB {
                warn!(
                    target: EVENT_TARGET,
                    filename = entry.file_name().to_str().unwrap_or_default(),
                    "new.unexpected_file"
                );
            }
        }

        snapshots.sort();

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));

        if snapshots.is_empty() {
            return Err(OpenError::NoStableSnapshot);
        }

        Ok(RocksDB {
            snapshots,
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn empty(dir: &Path) -> Result<RocksDB, OpenError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        Ok(RocksDB {
            snapshots: vec![],
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn from_snapshot(dir: &Path, epoch: Epoch) -> Result<RocksDB, OpenError> {
        let mut opts = Options::default();
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        Ok(RocksDB {
            snapshots: vec![epoch],
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join(PathBuf::from(format!("{epoch:?}"))))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn unsafe_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }
}

impl Store for RocksDB {
    type Error = rocksdb::Error;

    fn tip(&self) -> Result<Point, Self::Error> {
        Ok(self
            .db
            .get(KEY_TIP)?
            .map(|bytes| cbor::decode(&bytes))
            .transpose()
            .expect("unable to decode database's tip")
            .unwrap_or_else(|| {
                panic!("no database tip. Did you forget to 'import' a snapshot first?")
            }))
    }

    fn save(
        &'_ self,
        point: &'_ Point,
        issuer: Option<&'_ PoolId>,
        add: Columns<
            impl Iterator<Item = utxo::Add>,
            impl Iterator<Item = pools::Add>,
            impl Iterator<Item = accounts::Add>,
        >,
        remove: Columns<
            impl Iterator<Item = utxo::Remove>,
            impl Iterator<Item = pools::Remove>,
            impl Iterator<Item = accounts::Remove>,
        >,
    ) -> Result<(), Self::Error> {
        let batch = self.db.transaction();

        let tip: Option<Point> = batch.get(KEY_TIP)?.map(|bytes| {
            cbor::decode(&bytes).unwrap_or_else(|e| {
                panic!(
                    "unable to decode database tip ({}): {e:?}",
                    hex::encode(&bytes)
                )
            })
        });

        match (point, tip) {
            (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                info!(target: EVENT_TARGET, ?point, "save.point_already_known");
            }
            _ => {
                batch.put(KEY_TIP, as_value(point))?;
                if let Some(issuer) = issuer {
                    slots::rocksdb::put(
                        &batch,
                        &point.slot_or_default(),
                        slots::Row::new(*issuer),
                    )?;
                }
                utxo::rocksdb::add(&batch, add.utxo)?;
                pools::rocksdb::add(&batch, add.pools)?;
                accounts::rocksdb::add(&batch, add.accounts)?;
                utxo::rocksdb::remove(&batch, remove.utxo)?;
                pools::rocksdb::remove(&batch, remove.pools)?;
                accounts::rocksdb::remove(&batch, remove.accounts)?;
            }
        }

        batch.commit()
    }

    fn most_recent_snapshot(&'_ self) -> Epoch {
        self.snapshots
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("called 'most_recent_snapshot' on empty database?!"))
    }

    fn next_snapshot(&'_ mut self, epoch: Epoch) -> Result<(), Self::Error> {
        let snapshot = self.snapshots.last().map(|s| s + 1).unwrap_or(epoch);
        if snapshot == epoch {
            let path = self.dir.join(snapshot.to_string());
            checkpoint::Checkpoint::new(&self.db)?.create_checkpoint(path)?;
            self.snapshots.push(snapshot);
        } else {
            info!(target: EVENT_TARGET, %epoch, "next_snapshot.already_known");
        }
        Ok(())
    }

    fn pots(&self) -> Result<pots::Row, Self::Error> {
        pots::rocksdb::get(&self.db)
    }

    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, Self::Error> {
        pools::rocksdb::get(&self.db, pool)
    }

    fn with_utxo(&self, with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), utxo::rocksdb::PREFIX, with)
    }

    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), pools::rocksdb::PREFIX, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(accounts::Iter<'_, '_>),
    ) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), accounts::rocksdb::PREFIX, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(slots::Iter<'_, '_>),
    ) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), slots::rocksdb::PREFIX, with)
    }
}

/// An generic column iterator, provided that rows from the column are (de)serialisable.
fn with_prefix_iterator<
    K: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    V: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    DB,
>(
    db: rocksdb::Transaction<'_, DB>,
    prefix: [u8; PREFIX_LEN],
    mut with: impl FnMut(IterBorrow<'_, '_, K, Option<V>>),
) -> Result<(), rocksdb::Error> {
    let mut iterator =
        iter_borrow::new::<PREFIX_LEN, _, _>(db.prefix_iterator(prefix).map(|item| {
            // FIXME: clarify what kind of errors can come from the database at this point.
            // We are merely iterating over a collection.
            item.unwrap_or_else(|e| panic!("unexpected database error: {e:?}"))
        }));

    with(iterator.as_iter_borrow());

    for (k, v) in iterator.into_iter_updates() {
        match v {
            Some(v) => db.put(k, as_value(v)),
            None => db.delete(k),
        }?;
    }

    db.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::kernel::{
        encode_bech32, output_lovelace, output_stake_credential, Hash, Lovelace, PoolParams,
        StakeCredential,
    };
    use num::BigRational;
    use serde::ser::SerializeStruct;
    use std::collections::BTreeMap;

    const LEDGER_DB: &str = "../../ledger.db";

    struct Snapshot {
        keys: BTreeMap<Hash<28>, AccountState>,
        scripts: BTreeMap<Hash<28>, AccountState>,
        pools: BTreeMap<PoolId, PoolState>,
    }

    impl serde::Serialize for Snapshot {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("Snapshot", 3)?;
            s.serialize_field("keys", &self.keys)?;
            s.serialize_field("scripts", &self.scripts)?;
            let mut pools = self
                .pools
                .iter()
                .map(|(k, v)| (encode_bech32("pool", &k[..]).unwrap(), v))
                .collect::<Vec<_>>();
            pools.sort_by(|a, b| a.0.cmp(&b.0));
            s.serialize_field(
                "stakePoolParameters",
                &pools.into_iter().collect::<BTreeMap<String, &PoolState>>(),
            )?;
            s.end()
        }
    }

    struct AccountState {
        lovelace: Lovelace,
        pool: PoolId,
    }

    impl serde::Serialize for AccountState {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("AccountState", 2)?;
            s.serialize_field("lovelace", &self.lovelace)?;
            s.serialize_field("pool", &encode_bech32("pool", &self.pool[..]).unwrap())?;
            s.end()
        }
    }

    struct PoolState {
        blocks_count: u64,
        relative_stake: BigRational,
        parameters: PoolParams,
    }

    impl serde::Serialize for PoolState {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("PoolState", 3)?;
            s.serialize_field("blocksCount", &self.blocks_count)?;
            s.serialize_field("relativeStake", &serialize_ratio(&self.relative_stake))?;
            s.serialize_field("parameters", &self.parameters)?;
            s.end()
        }
    }

    fn serialize_ratio(r: &BigRational) -> String {
        format!("{}/{}", r.numer(), r.denom())
    }

    fn new_preprod_snapshot(epoch: Epoch, total_stake: Lovelace) -> Snapshot {
        let db = RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), epoch).unwrap();

        let mut accounts = BTreeMap::new();
        db.with_accounts(|rows| {
            for (credential, row) in rows {
                if let Some(account) = row.borrow() {
                    if let Some(pool) = account.delegatee {
                        accounts.insert(
                            credential,
                            AccountState {
                                pool,
                                lovelace: account.rewards,
                            },
                        );
                    }
                }
            }
        })
        .unwrap();

        db.with_utxo(|rows| {
            for (_, row) in rows {
                if let Some(output) = row.borrow() {
                    if let Some(credential) = output_stake_credential(output) {
                        let value = output_lovelace(output);
                        accounts
                            .entry(credential)
                            .and_modify(|account| account.lovelace += value);
                    }
                }
            }
        })
        .unwrap();

        let mut pools: BTreeMap<PoolId, PoolState> = BTreeMap::new();
        db.with_pools(|rows| {
            for (pool, row) in rows {
                if let Some(row) = row.borrow() {
                    pools.insert(
                        pool,
                        PoolState {
                            relative_stake: BigRational::from_integer(0.into()),
                            blocks_count: 0,
                            parameters: row.current_params.clone(),
                        },
                    );
                }
            }
        })
        .unwrap();

        let mut accounts_scripts = BTreeMap::new();
        let mut accounts_keys = BTreeMap::new();
        for (credential, account) in accounts.into_iter() {
            // TODO: Add active stake

            pools.entry(account.pool).and_modify(|st| {
                st.relative_stake += BigRational::new(account.lovelace.into(), total_stake.into());
            });

            if pools.contains_key(&account.pool) {
                match credential {
                    StakeCredential::ScriptHash(script) => {
                        accounts_scripts.insert(script, account);
                    }
                    StakeCredential::AddrKeyhash(key) => {
                        accounts_keys.insert(key, account);
                    }
                }
            }
        }

        db.with_block_issuers(|rows| {
            for (_, row) in rows {
                if let Some(issuer) = row.borrow() {
                    pools
                        .entry(issuer.slot_leader)
                        .and_modify(|pool| pool.blocks_count += 1);
                }
            }
        })
        .unwrap();

        Snapshot {
            keys: accounts_keys,
            scripts: accounts_scripts,
            pools,
        }
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_165() {
        let snapshot = new_preprod_snapshot(165, 31112793878086059);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_166() {
        let snapshot = new_preprod_snapshot(166, 31119117576599736);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_167() {
        let snapshot = new_preprod_snapshot(167, 31125529153288004);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_168() {
        let snapshot = new_preprod_snapshot(168, 31131906732704284);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_169() {
        let snapshot = new_preprod_snapshot(169, 31138307137790052);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_170() {
        let snapshot = new_preprod_snapshot(170, 31144692895041950);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_171() {
        let snapshot = new_preprod_snapshot(171, 31150987758289950);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_172() {
        let snapshot = new_preprod_snapshot(172, 31157306038590590);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_173() {
        let snapshot = new_preprod_snapshot(173, 31163830817196470);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_174() {
        let snapshot = new_preprod_snapshot(174, 31170156774217170);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_175() {
        let snapshot = new_preprod_snapshot(175, 31176424921600024);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_176() {
        let snapshot = new_preprod_snapshot(176, 31183205224639780);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_177() {
        let snapshot = new_preprod_snapshot(177, 31189917429258484);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_178() {
        let snapshot = new_preprod_snapshot(178, 31196606373586140);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_179() {
        let snapshot = new_preprod_snapshot(179, 31203363503294896);
        insta::assert_json_snapshot!(snapshot);
    }
}
