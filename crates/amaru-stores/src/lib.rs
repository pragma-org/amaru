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

pub mod in_memory;
#[cfg(feature = "rocksdb")]
pub mod rocksdb;

#[cfg(test)]
pub mod tests {
    use amaru_kernel::{
        network::NetworkName,
        tests::{any_pool_id, any_pool_params, any_proposal_id},
        Anchor, EraHistory, Hash, MemoizedTransactionOutput, Point, PoolId, PoolParams, ProposalId,
        Slot, StakeCredential, TransactionInput,
    };
    use amaru_ledger::{
        state::diff_bind,
        store::{
            columns::{
                accounts::{self, tests::any_stake_credential},
                cc_members, dreps,
                proposals::{self},
                slots::tests::any_slot,
                utxo::tests::{any_memoized_transaction_output, any_txin},
            },
            Columns, ReadStore, Store, StoreError, TransactionalContext,
        },
    };
    use proptest::{prelude::Strategy, strategy::ValueTree, test_runner::TestRunner};
    use slot_arithmetic::Epoch;

    #[cfg(not(target_os = "windows"))]
    #[derive(Debug, Clone)]
    pub struct Fixture {
        pub account_key: StakeCredential,
        pub account_row: accounts::Row,
        pub txin: TransactionInput,
        pub output: MemoizedTransactionOutput,
        pub pool_params: PoolParams,
        pub pool_epoch: Epoch,
        pub drep_key: StakeCredential,
        pub drep_row: dreps::Row,
        pub proposal_key: ProposalId,
        pub proposal_row: proposals::Row,
        pub cc_member_key: StakeCredential,
        pub cc_member_row: cc_members::Row,
        pub slot: Slot,
        pub slot_leader: PoolId,
    }

    #[cfg(target_os = "windows")]
    #[derive(Debug, Clone)]
    pub struct Fixture {
        pub account_key: StakeCredential,
        pub account_row: accounts::Row,
        pub txin: TransactionInput,
        pub output: MemoizedTransactionOutput,
        pub pool_params: PoolParams,
        pub pool_epoch: Epoch,
        pub drep_key: StakeCredential,
        pub drep_row: dreps::Row,
        pub cc_member_key: StakeCredential,
        pub cc_member_row: cc_members::Row,
        pub slot: Slot,
        pub slot_leader: PoolId,
    }

    pub fn add_test_data_to_store(
        store: &impl Store,
        era_history: &EraHistory,
        runner: &mut TestRunner,
    ) -> Result<Fixture, StoreError> {
        use diff_bind::Resettable;

        // utxos
        let txin = any_txin().new_tree(runner).unwrap().current();
        let output = any_memoized_transaction_output()
            .new_tree(runner)
            .unwrap()
            .current();
        let utxos_iter = std::iter::once((txin.clone(), output.clone()));

        // accounts
        let account_key = any_stake_credential().new_tree(runner).unwrap().current();
        let account_key_clone = account_key.clone();

        let account_row = amaru_ledger::store::columns::accounts::tests::any_row()
            .new_tree(runner)
            .unwrap()
            .current();

        let delegatee = match &account_row.delegatee {
            Some(pool_id) => Resettable::Set(*pool_id),
            None => Resettable::Reset,
        };

        let drep = match &account_row.drep {
            Some(drep_pair) => Resettable::Set(drep_pair.clone()),
            None => Resettable::Reset,
        };

        let rewards = Some(account_row.rewards);
        let deposit = account_row.deposit;

        let accounts_iter =
            std::iter::once((account_key_clone, (delegatee, drep, rewards, deposit)));

        // pools
        let pool_params = any_pool_params().new_tree(runner).unwrap().current();
        let pool_epoch = Epoch::from(0u64);

        let pools_iter = std::iter::once((pool_params.clone(), pool_epoch));

        // dreps
        let drep_key = any_stake_credential().new_tree(runner).unwrap().current();
        let mut drep_row = amaru_ledger::store::columns::dreps::tests::any_row()
            .new_tree(runner)
            .unwrap()
            .current();

        if drep_row.anchor.is_none() {
            drep_row.anchor = Some(Anchor {
                url: "https://example.com".to_string(),
                content_hash: Hash::from([0u8; 32]),
            });
        }
        drep_row.previous_deregistration = None;
        drep_row.last_interaction = None;

        let anchor = drep_row.anchor.clone().expect("Expected anchor to be Some");
        let deposit = drep_row.deposit;
        let registered_at = drep_row.registered_at;

        let drep_epoch = era_history
            .slot_to_epoch(
                registered_at.transaction.slot,
                registered_at.transaction.slot,
            )
            .expect("Failed to convert slot to epoch");

        let drep_iter = std::iter::once((
            drep_key.clone(),
            (
                Resettable::Set(anchor),
                Some((deposit, registered_at)),
                drep_epoch,
            ),
        ));

        // proposals (Does not generate proposal row on Windows due to stack overflow)
        #[cfg(not(target_os = "windows"))]
        let (proposal_iter, proposal_key, proposal_row) = {
            let proposal_key = any_proposal_id().new_tree(runner).unwrap().current();
            let proposal_row = amaru_ledger::store::columns::proposals::tests::any_row()
                .new_tree(runner)
                .unwrap()
                .current();
            (
                std::iter::once((proposal_key.clone(), proposal_row.clone())),
                proposal_key,
                proposal_row,
            )
        };

        #[cfg(target_os = "windows")]
        let proposal_iter = std::iter::empty();

        let votes_iter = std::iter::empty();

        // cc_members
        let cc_member_key = any_stake_credential().new_tree(runner).unwrap().current();
        let mut cc_member_row = amaru_ledger::store::columns::cc_members::tests::any_row()
            .new_tree(runner)
            .unwrap()
            .current();

        // Ensure hot_credential is always Some
        cc_member_row
            .hot_credential
            .get_or_insert_with(|| any_stake_credential().new_tree(runner).unwrap().current());

        let hot_credential = cc_member_row.hot_credential.clone().unwrap();

        let cc_members_iter =
            std::iter::once((cc_member_key.clone(), Resettable::Set(hot_credential)));

        let slot = any_slot().new_tree(runner).unwrap().current();
        let point = Point::Specific(slot.into(), Hash::from([0u8; 32]).to_vec());
        let slot_leader = any_pool_id().new_tree(runner).unwrap().current();

        let era_history = (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();

        {
            let context = store.create_transaction();

            context.save(
                &point,
                Some(&slot_leader),
                Columns {
                    utxo: utxos_iter,
                    pools: pools_iter,
                    accounts: accounts_iter,
                    dreps: drep_iter,
                    cc_members: cc_members_iter,
                    proposals: proposal_iter,
                    votes: votes_iter,
                },
                Columns::empty(),
                std::iter::empty(),
                &era_history,
            )?;

            context.commit()?;
        }

        let stored_account_row = {
            let context = store.create_transaction();
            let mut result = None;
            context.with_accounts(|mut accounts| {
                result = accounts
                    .find(|(key, _)| *key == account_key)
                    .and_then(|(_, row)| row.borrow().clone());
            })?;
            let value = result.ok_or_else(|| {
                StoreError::Internal("Failed to retrieve account after seeding".into())
            })?;
            context.commit()?;
            value
        };

        Ok(Fixture {
            account_key,
            account_row: stored_account_row,
            txin,
            output,
            pool_params,
            pool_epoch,
            drep_key,
            drep_row,
            #[cfg(not(target_os = "windows"))]
            proposal_key,
            #[cfg(not(target_os = "windows"))]
            proposal_row,
            cc_member_key,
            cc_member_row,
            slot,
            slot_leader,
        })
    }

    pub fn test_read_utxo(store: &impl ReadStore, fixture: &Fixture) {
        let result = store
            .utxo(&fixture.txin)
            .expect("failed to read UTXO from store");

        assert_eq!(
            result,
            Some(fixture.output.clone()),
            "UTXO did not match fixture output"
        );
    }

    pub fn test_read_account(store: &impl ReadStore, fixture: &Fixture) {
        let stored_account = store
            .account(&fixture.account_key)
            .expect("failed to read account from store");

        assert!(
            stored_account.is_some(),
            "account not found in store for fixture key"
        );

        let stored_account = stored_account.unwrap();

        assert_eq!(
            stored_account.delegatee, fixture.account_row.delegatee,
            "delegatee mismatch"
        );
        assert_eq!(
            stored_account.drep, fixture.account_row.drep,
            "drep mismatch"
        );
        assert_eq!(
            stored_account.rewards, fixture.account_row.rewards,
            "rewards mismatch"
        );
        assert_eq!(
            stored_account.deposit, fixture.account_row.deposit,
            "deposit mismatch"
        );
    }

    pub fn test_read_pool(store: &impl ReadStore, fixture: &Fixture) {
        let pool_id = fixture.pool_params.id;
        let stored_pool = store
            .pool(&pool_id)
            .expect("failed to read pool from store");

        assert!(
            stored_pool.is_some(),
            "pool not found in store for fixture id"
        );

        let stored_pool = stored_pool.unwrap();

        assert_eq!(
            stored_pool.current_params, fixture.pool_params,
            "current pool params mismatch"
        );
        assert_eq!(
            stored_pool.future_params,
            vec![],
            "future pool params mismatch"
        );
    }

    pub fn test_read_drep(store: &impl ReadStore, fixture: &Fixture) {
        let stored_drep = store
            .iter_dreps()
            .expect("failed to iterate dreps")
            .find(|(key, _)| key == &fixture.drep_key)
            .map(|(_, row)| row)
            .expect("drep not found in store");

        assert_eq!(
            stored_drep.anchor, fixture.drep_row.anchor,
            "drep anchor mismatch"
        );
        assert_eq!(
            stored_drep.deposit, fixture.drep_row.deposit,
            "drep deposit mismatch"
        );
        assert_eq!(
            stored_drep.registered_at, fixture.drep_row.registered_at,
            "drep registration time mismatch"
        );
        assert_eq!(
            stored_drep.last_interaction, fixture.drep_row.last_interaction,
            "drep last interaction mismatch"
        );

        match (
            &stored_drep.previous_deregistration,
            &fixture.drep_row.previous_deregistration,
        ) {
            (Some(a), Some(b)) => assert_eq!(a, b, "drep previous deregistration mismatch"),
            (None, None) => {}
            (left, right) => panic!(
                "Mismatch in previous_deregistration: left = {:?}, right = {:?}",
                left, right
            ),
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn test_read_proposal(store: &impl Store, fixture: &Fixture) {
        let stored_proposal = store
            .iter_proposals()
            .expect("failed to iterate proposals")
            .find(|(key, _)| key == &fixture.proposal_key)
            .map(|(_, row)| row);

        assert!(
            stored_proposal.is_some(),
            "proposal not found in store for fixture key"
        );

        let stored_proposal = stored_proposal.unwrap();

        assert_eq!(
            stored_proposal.proposed_in, fixture.proposal_row.proposed_in,
            "proposal proposed_in mismatch"
        );
        assert_eq!(
            stored_proposal.valid_until, fixture.proposal_row.valid_until,
            "proposal valid_until mismatch"
        );
        assert_eq!(
            stored_proposal.proposal, fixture.proposal_row.proposal,
            "proposal data mismatch"
        );
    }

    pub fn test_remove_utxo(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let point = Point::Origin;

        let remove = Columns {
            utxo: std::iter::once(fixture.txin.clone()),
            pools: std::iter::empty(),
            accounts: std::iter::empty(),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        };
        let era_history = (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            &era_history,
        )?;
        context.commit()?;

        assert_eq!(
            store.utxo(&fixture.txin).expect("utxo lookup failed"),
            None,
            "utxo was not properly removed"
        );

        Ok(())
    }

    pub fn test_remove_account(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let point = Point::Origin;

        let remove = Columns {
            utxo: std::iter::empty(),
            pools: std::iter::empty(),
            accounts: std::iter::once(fixture.account_key.clone()),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        };

        let era_history = (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            &era_history,
        )?;
        context.commit()?;

        assert_eq!(store.account(&fixture.account_key)?, None);

        Ok(())
    }

    pub fn test_remove_pool(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let point = Point::Origin;

        let remove = Columns {
            utxo: std::iter::empty(),
            pools: std::iter::once((fixture.pool_params.id, fixture.pool_epoch)),
            accounts: std::iter::empty(),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        };
        let era_history = (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            &era_history,
        )?;
        context.commit()?;

        assert!(
            store
                .pool(&fixture.pool_params.id)?
                .expect("Expected pool row")
                .future_params
                .iter()
                .any(|(p, e)| p.is_none() && *e == fixture.pool_epoch),
            "Expected pool to be scheduled for removal"
        );

        Ok(())
    }

    pub fn test_remove_drep(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let point = Point::Origin;

        let drep_registered_at = store
            .iter_dreps()?
            .find(|(key, _)| *key == fixture.drep_key)
            .map(|(_, row)| row.registered_at)
            .ok_or_else(|| StoreError::Internal("DRep not found before removal".into()))?;

        let remove = Columns {
            utxo: std::iter::empty(),
            pools: std::iter::empty(),
            accounts: std::iter::empty(),
            dreps: std::iter::once((fixture.drep_key.clone(), drep_registered_at)),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        };

        assert!(
            store.iter_dreps()?.any(|(key, _)| key == fixture.drep_key),
            "DRep not present before removal"
        );

        let era_history = (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            &era_history,
        )?;
        context.commit()?;

        let maybe_drep_row = store
            .iter_dreps()?
            .find(|(key, _)| *key == fixture.drep_key)
            .map(|(_, row)| row);

        let drep_row = maybe_drep_row.ok_or_else(|| {
            StoreError::Internal("DRep row not found after supposed deregistration".into())
        })?;

        assert_eq!(
            drep_row.previous_deregistration,
            Some(drep_registered_at),
            "DRep was not marked as deregistered"
        );

        Ok(())
    }

    pub fn test_refund_account(
        store: &impl Store,
        fixture: &Fixture,
        runner: &mut TestRunner,
    ) -> Result<(), StoreError> {
        let refund_amount = 100;

        let context = store.create_transaction();

        let mut result = None;
        context.with_accounts(|mut accounts| {
            result = accounts
                .find(|(key, _)| *key == fixture.account_key)
                .and_then(|(_, row)| row.borrow().as_ref().map(|acc| acc.rewards));
        })?;

        let rewards_before =
            result.ok_or_else(|| StoreError::Internal("Missing account before refund".into()))?;

        let unrefunded = context.refund(&fixture.account_key, refund_amount)?;
        assert_eq!(unrefunded, 0, "Refund to existing account should succeed");
        context.commit()?;

        let rewards_after = {
            let context = store.create_transaction();
            let mut result = None;
            context.with_accounts(|mut accounts| {
                result = accounts
                    .find(|(key, _)| *key == fixture.account_key)
                    .and_then(|(_, row)| row.borrow().as_ref().map(|acc| acc.rewards));
            })?;
            context.commit()?;

            result.ok_or_else(|| StoreError::Internal("Missing account after refund".into()))?
        };

        assert_eq!(
            rewards_after,
            rewards_before + refund_amount,
            "Rewards should increase by refund amount"
        );

        {
            let unknown = any_stake_credential().new_tree(runner).unwrap().current();
            assert_ne!(unknown, fixture.account_key);

            let context = store.create_transaction();
            let refund_amount = 123;
            let unrefunded = context.refund(&unknown, refund_amount)?;
            assert_eq!(
                unrefunded, refund_amount,
                "Missing account should return full refund amount"
            );
            context.commit()?;
        }

        Ok(())
    }

    pub fn test_epoch_transition(store: &impl Store) -> Result<(), StoreError> {
        use amaru_ledger::store::EpochTransitionProgress;

        let context = store.create_transaction();

        let from = None;
        let to = Some(EpochTransitionProgress::EpochEnded);

        let success = context.try_epoch_transition(from.clone(), to.clone())?;
        assert!(
            success,
            "Expected epoch transition to succeed when previous state matches"
        );

        let repeat = context.try_epoch_transition(from, to)?;
        assert!(
            !repeat,
            "Expected second transition from outdated state to fail"
        );
        context.commit()?;

        Ok(())
    }

    pub fn test_slot_updated(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let issuers: Vec<_> = store.iter_block_issuers()?.collect();

        let found = issuers
            .iter()
            .any(|(slot, row)| *slot == fixture.slot && row.slot_leader == fixture.slot_leader);

        assert!(
            found,
            "expected slot {:?} with issuer {:?} not found",
            fixture.slot, fixture.slot_leader
        );

        Ok(())
    }
}
