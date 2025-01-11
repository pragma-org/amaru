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
        StakeCredential, ACTIVE_SLOT_COEFF_INVERSE, MAX_LOVELACE_SUPPLY,
        MONETARY_EXPANSION_RATE_DEN, MONETARY_EXPANSION_RATE_NUM, OPTIMAL_STAKE_POOLS_COUNT,
        PLEDGE_INFLUENCE_DEN, PLEDGE_INFLUENCE_NUM, SHELLEY_EPOCH_LENGTH, TREASURY_TAX_DEN,
        TREASURY_TAX_NUM,
    };
    use num::{
        traits::{One, Zero},
        BigInt, BigRational,
    };
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

    #[derive(Debug)]
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

    #[derive(Debug)]
    struct PoolState {
        blocks_count: u64,
        relative_stake: BigRational,
        parameters: PoolParams,
        rewards_pot: BigInt,
        leader_rewards: BigInt,
    }

    impl PoolState {
        pub fn owner_stake(&self, accounts: &BTreeMap<Hash<28>, AccountState>) -> BigInt {
            self.parameters
                .owners
                .iter()
                .fold(BigInt::ZERO, |total, owner| match accounts.get(owner) {
                    Some(account) if account.pool == self.parameters.id => {
                        total + BigInt::from(account.lovelace)
                    }
                    _ => total,
                })
        }

        pub fn apparent_performances(
            &self,
            blocks_count: &BigInt,
            active_stake: &BigInt,
            total_stake: &BigInt,
        ) -> BigRational {
            if self.relative_stake.is_zero() {
                BigRational::zero()
            } else {
                let beta = BigRational::new(
                    BigInt::from(self.blocks_count),
                    blocks_count.max(&BigInt::one()).clone(),
                );
                let sigma_a = &self.relative_stake * total_stake / active_stake;
                beta / sigma_a
            }
        }

        // Optimal (i.e. maximum) rewards for a pool assuming it is fully saturated and producing
        // its expected number of blocks.
        //
        // The results is then used to calculate the _actual rewards_ based on the pool
        // performances and its actual saturation level.
        pub fn optimal_rewards(&self, available_rewards: &BigInt, total_stake: &BigInt) -> BigInt {
            let one = BigRational::from_integer(1.into());
            let a0 = BigRational::new(PLEDGE_INFLUENCE_NUM.into(), PLEDGE_INFLUENCE_DEN.into());
            let z0 = BigRational::new(1.into(), OPTIMAL_STAKE_POOLS_COUNT.into());

            let pledge = BigRational::new(self.parameters.pledge.into(), total_stake.clone());

            let relative_pledge = (&z0).min(&pledge);
            let relative_stake = (&z0).min(&self.relative_stake);

            // factor1 = coinToRational r / (1 + unboundRational a0)
            let factor1 = BigRational::from_integer(available_rewards.clone()) / (one + &a0);

            // factor4 = (z0 - sigma') / z0
            let factor4 = (&z0 - relative_stake) / &z0;

            // factor3 = (sigma' - p' * factor4) / z0
            let factor3 = (relative_stake - relative_pledge * factor4) / &z0;

            // factor2 = sigma' + p' * unboundRational a0 * factor3
            let factor2 = relative_stake + relative_pledge * &a0 * factor3;

            (factor1 * factor2).floor().to_integer()
        }

        pub fn pool_rewards(
            &self,
            available_rewards: &BigInt,
            blocks_count: &BigInt,
            active_stake: &BigInt,
            total_stake: &BigInt,
            owner_stake: &BigInt,
        ) -> BigInt {
            if &BigInt::from(self.parameters.pledge) <= owner_stake {
                (self.apparent_performances(blocks_count, active_stake, total_stake)
                    * self.optimal_rewards(available_rewards, total_stake))
                .floor()
                .to_integer()
            } else {
                BigInt::ZERO
            }
        }

        pub fn leader_rewards(
            &self,
            pool_rewards: BigInt,
            owner_stake: BigInt,
            total_stake: &BigInt,
        ) -> BigInt {
            let cost: BigInt = self.parameters.cost.into();

            let margin: BigRational = BigRational::new(
                self.parameters.margin.numerator.into(),
                self.parameters.margin.denominator.into(),
            );

            if pool_rewards <= cost {
                pool_rewards
            } else {
                let member_rewards: BigInt = pool_rewards - &cost;
                let owner_stake_ratio = if total_stake.is_zero() {
                    BigRational::zero()
                } else {
                    BigRational::new(owner_stake, total_stake.clone())
                };
                let margin_factor: BigRational = &margin
                    + (BigRational::from_integer(BigInt::from(1)) - &margin) * &owner_stake_ratio
                        / &self.relative_stake;
                cost + (margin_factor * member_rewards).floor().to_integer()
            }
        }
    }

    impl serde::Serialize for PoolState {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("PoolState", 3)?;
            s.serialize_field("blocksCount", &self.blocks_count)?;
            s.serialize_field("relativeStake", &serialize_ratio(&self.relative_stake))?;
            s.serialize_field("rewardsPot", &u64::try_from(&self.rewards_pot).unwrap())?;
            s.serialize_field(
                "leaderRewards",
                &u64::try_from(&self.leader_rewards).unwrap(),
            )?;
            s.serialize_field("parameters", &self.parameters)?;
            s.end()
        }
    }

    fn serialize_ratio(r: &BigRational) -> String {
        format!("{}/{}", r.numer(), r.denom())
    }

    fn new_preprod_snapshot(epoch: Epoch) -> Snapshot {
        let db_future = RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), epoch + 2).unwrap();

        let pots = db_future.pots().unwrap();

        let total_stake: BigInt = (MAX_LOVELACE_SUPPLY - pots.reserves).into();

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
                            rewards_pot: BigInt::ZERO,
                            leader_rewards: BigInt::ZERO,
                            parameters: row.current_params.clone(),
                        },
                    );
                }
            }
        })
        .unwrap();

        let mut accounts_scripts = BTreeMap::new();
        let mut accounts_keys = BTreeMap::new();
        let mut active_stake: BigInt = BigInt::ZERO;
        for (credential, account) in accounts.into_iter() {
            pools.entry(account.pool).and_modify(|st| {
                active_stake += account.lovelace;
                st.relative_stake += BigRational::new(account.lovelace.into(), total_stake.clone());
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

        let mut blocks_count = 0;
        db_future
            .with_block_issuers(|rows| {
                for (_, row) in rows {
                    if let Some(issuer) = row.borrow() {
                        blocks_count += 1;
                        pools
                            .entry(issuer.slot_leader)
                            .and_modify(|pool| pool.blocks_count += 1);
                    }
                }
            })
            .unwrap();

        assert_eq!(
            active_stake,
            BigInt::from(331086649066019_u64),
            "active_stake"
        );

        // The monetary expansion value, a.k.a ρ
        let monetary_expansion = BigRational::new(
            MONETARY_EXPANSION_RATE_NUM.into(),
            MONETARY_EXPANSION_RATE_DEN.into(),
        );

        // The treasury tax value, a.k.a τ
        let treasury_tax = BigRational::new(TREASURY_TAX_NUM.into(), TREASURY_TAX_DEN.into());

        // The ratio of block produced over the number of expected blocks (a.k.a η).
        // Expected blocks are fixed for an epoch and depends on protocol parameters.
        //
        // Note that, this ratio **may be greater than one**, since the slot election is random and
        // only averages towards a certain number per epoch.
        let epoch_blocks = BigRational::new(
            BigInt::from(blocks_count) * ACTIVE_SLOT_COEFF_INVERSE,
            SHELLEY_EPOCH_LENGTH.into(),
        );
        assert_eq!(epoch_blocks, BigRational::new(541.into(), 720.into()), "η",);

        // The amount of Ada taken out of the reserves as incentivies at this particular epoch
        // (a.k.a ΔR1).
        // It is so-to-speak, the monetary inflation of the network that fuels the incentives.
        let incentives = (BigRational::from_integer(1.into()).min(epoch_blocks.clone())
            * monetary_expansion
            * BigRational::from_integer(pots.reserves.into()))
        .floor()
        .to_integer();

        assert_eq!(incentives, 31333093707160_usize.into(), "ΔR1",);

        // Total rewards available before the treasury tax.
        let total_rewards = incentives + BigInt::from(pots.fees);
        assert_eq!(total_rewards, 31349235009257_usize.into(), "r_pot");

        // Treasury tax
        let rewards_for_treasury = (treasury_tax * &total_rewards).floor().to_integer();
        assert_eq!(rewards_for_treasury, BigInt::from(6269847001851_usize), "τ");

        // Remaining rewards available to stake pools (and delegators)
        let remaining_rewards: BigInt = total_rewards - rewards_for_treasury;

        pools.iter_mut().for_each(|(_, pool)| {
            let rewards_pot = pool.pool_rewards(
                &remaining_rewards,
                &BigInt::from(blocks_count),
                &active_stake,
                &total_stake,
                &pool.owner_stake(&accounts_keys),
            );

            pool.rewards_pot = rewards_pot.clone();
            pool.leader_rewards =
                pool.leader_rewards(rewards_pot, pool.owner_stake(&accounts_keys), &total_stake);

            // Reset blocks count. We used the block counts at epoch + 2 for the rewards
            // calculation, but the snapshot refers to pools at the current epoch. So we need
            // to replace block counts now that it is no longer needed for rewards calculation.
            pool.blocks_count = 0;
        });

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
    fn compare_preprod_snapshot_163() {
        let snapshot = new_preprod_snapshot(163);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_164() {
        let snapshot = new_preprod_snapshot(164);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_165() {
        let snapshot = new_preprod_snapshot(165);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_166() {
        let snapshot = new_preprod_snapshot(166);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_167() {
        let snapshot = new_preprod_snapshot(167);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_168() {
        let snapshot = new_preprod_snapshot(168);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_169() {
        let snapshot = new_preprod_snapshot(169);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_170() {
        let snapshot = new_preprod_snapshot(170);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_171() {
        let snapshot = new_preprod_snapshot(171);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_172() {
        let snapshot = new_preprod_snapshot(172);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_173() {
        let snapshot = new_preprod_snapshot(173);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_174() {
        let snapshot = new_preprod_snapshot(174);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_175() {
        let snapshot = new_preprod_snapshot(175);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_176() {
        let snapshot = new_preprod_snapshot(176);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_177() {
        let snapshot = new_preprod_snapshot(177);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_178() {
        let snapshot = new_preprod_snapshot(178);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_179() {
        let snapshot = new_preprod_snapshot(179);
        insta::assert_json_snapshot!(snapshot);
    }
}
