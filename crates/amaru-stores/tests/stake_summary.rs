// Copyright 2026 PRAGMA
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

use amaru_kernel::{
    CertificatePointer, Credential, DRep, Epoch, Hash, Network, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, PoolId, PoolParams, RewardAccount, UnitRationalNumber,
};
use amaru_ledger::{
    PoolState,
    epoch_transition::pools_updates::PoolCertificate,
    store,
    store::{Columns, Snapshot, Store, TransactionalContext, columns::accounts},
    summary::{governance::GovernanceSummary, stake_distribution::StakeSummary},
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

#[test]
fn a_retiring_pool_has_no_fallback_drep() {
    let setup = create_setup();

    // Retire the pool at the next epoch
    setup.update_pool(|pool| pool.pending_certificates.append(PoolCertificate::Retirement(setup.epoch + 1)));

    // Get the pool state from the latest snapshot
    let snapshot = setup.take_snapshot();
    let pool = setup.get_pool_state(snapshot);

    // Check that the fallback DRep has been removed since the pool is retiring at the next epoch.
    assert_eq!(pool.fallback_drep, None, "a pool retired at the next boundary must have no fallback DRep",);
}

#[test]
fn the_pool_fallback_drep_uses_the_reward_account_effective_at_next_boundary() {
    let setup = create_setup();
    let new_reward_credential = Credential::KeyHash(Hash::new([2; 28]));

    // Save a new account for rewards
    setup.save_bootstrap_account(
        new_reward_credential,
        accounts::Row { drep: Some((DRep::NoConfidence, setup.registered_at)), ..Default::default() },
    );

    // Update the pool so that it uses a new rewards account
    let updated_parameters = PoolParams {
        reward_account: RewardAccount::new(Network::Testnet, new_reward_credential),
        ..setup.parameters.clone()
    };
    setup.update_pool(|pool| pool.pending_certificates.append(updated_parameters.clone()));

    // Get the pool state from the latest snapshot
    let snapshot = setup.take_snapshot();
    let pool = setup.get_pool_state(snapshot);

    // Check that the fallback DRep defaults to the vote of the new reward account.
    assert_eq!(
        pool.fallback_drep,
        Some(DRep::NoConfidence),
        "the fallback must use the vote of the effective reward account at the next boundary",
    );
}

#[test]
fn a_pool_without_pending_changes_keeps_its_fallback_drep() {
    let setup = create_setup();
    let pool = setup.get_pool_state(setup.take_snapshot());

    assert_eq!(pool.fallback_drep, Some(DRep::Abstain));
}

#[test]
fn a_re_registered_pool_with_the_same_reward_account_keeps_its_fallback_drep() {
    let setup = create_setup();
    let updated_parameters = PoolParams { cost: 100, ..setup.parameters.clone() };
    setup.update_pool(|pool| pool.pending_certificates.append(updated_parameters.clone()));
    let pool = setup.get_pool_state(setup.take_snapshot());

    assert_eq!(pool.fallback_drep, Some(DRep::Abstain));
}

// HELPERS

struct Setup {
    cfg: RocksDbConfig,
    store: RocksDB,
    epoch: Epoch,
    registered_at: CertificatePointer,
    reward_credential: Credential,
    pool_id: PoolId,
    parameters: PoolParams,
    _tmp_dir: tempfile::TempDir,
}

#[expect(clippy::expect_used)]
fn create_setup() -> Setup {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let cfg = RocksDbConfig::new(tmp_dir.path().into());
    let store = RocksDB::empty(&cfg).expect("open empty store");
    let epoch = Epoch::from(163);
    let registered_at = CertificatePointer::default();
    let reward_credential = Credential::KeyHash(Hash::new([1; 28]));
    let pool_id = PoolId::new([3; 28]);
    let parameters = PoolParams {
        id: pool_id,
        vrf: Hash::new([4; 32]),
        pledge: 0,
        cost: 0,
        margin: UnitRationalNumber::new(0, 1).expect("valid margin"),
        reward_account: RewardAccount::new(Network::Testnet, reward_credential),
        owners: Default::default(),
        relays: Vec::new(),
        metadata: None,
    };

    let setup = Setup { cfg, store, epoch, registered_at, reward_credential, pool_id, parameters, _tmp_dir: tmp_dir };
    setup.initialize();
    setup
}

#[expect(clippy::expect_used)]
impl Setup {
    //  Initialize the store with a default pool and a bootstrap account
    fn initialize(&self) {
        self.store
            .with_transaction(|context| {
                context.set_protocol_parameters(&PREPROD_DEFAULT_PROTOCOL_PARAMETERS)?;
                context.save(
                    &PREPROD_ERA_HISTORY,
                    &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                    None,
                    &crate::point(69_200_000),
                    None,
                    Columns {
                        utxo: std::iter::empty(),
                        pools: std::iter::once((
                            self.parameters.clone(),
                            self.registered_at,
                            PREPROD_DEFAULT_PROTOCOL_PARAMETERS.stake_pool_deposit,
                        )),
                        accounts: std::iter::empty(),
                        dreps: std::iter::empty(),
                        cc_members: std::iter::empty(),
                        proposals: std::iter::empty(),
                        votes: std::iter::empty(),
                    },
                    Columns::empty(),
                    std::iter::empty(),
                )
            })
            .expect("test pool");

        self.save_bootstrap_account(
            self.reward_credential,
            accounts::Row {
                pool: Some((self.pool_id, self.registered_at)),
                drep: Some((DRep::Abstain, self.registered_at)),
                rewards: 100,
                ..Default::default()
            },
        )
    }

    fn save_bootstrap_account(&self, credential: Credential, account_row: accounts::Row) {
        self.store.save_bootstrap_accounts([(credential, account_row)].into_iter()).expect("seed reward account")
    }

    // Take a snapshot for the current epoch and open it
    fn take_snapshot(&self) -> impl Snapshot {
        self.store.next_snapshot(self.epoch).expect("create snapshot");
        RocksDBHistoricalStores::for_epoch_with(&self.cfg, self.epoch).expect("open snapshot")
    }

    // Return a stake summary for the latest snapshot
    fn get_stake_summary(&self, snapshot: impl Snapshot) -> StakeSummary {
        let governance = GovernanceSummary {
            dreps: Default::default(),
            dreps_deposits: Default::default(),
            pools_deposits: Default::default(),
            cc_update: None,
        };
        StakeSummary::new(&snapshot, governance, NetworkName::Preprod, |_| {}).expect("stake summary")
    }

    // Get the test pool state from the stake summary
    fn get_pool_state(&self, snapshot: impl Snapshot) -> PoolState {
        let summary = self.get_stake_summary(snapshot);
        summary.pools.get(&self.pool_id).expect("snapshot retains pool").clone()
    }

    // Update all the pools
    fn update_pools(&self, with: impl FnMut(store::columns::pools::Iter<'_, '_>)) {
        self.store.with_transaction(|context| context.with_pools(with)).expect("update pools");
    }

    // Update the test pool
    fn update_pool(&self, with: impl Fn(&mut store::columns::pools::Row)) {
        self.update_pools(|mut pools| {
            let (id, mut row) = pools.next().expect("pool exists");
            assert_eq!(id, self.pool_id);
            with(row.as_mut().borrow_mut().as_mut().expect("pool row"));
        });
    }
}
