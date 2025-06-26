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
pub mod rocksdb;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub mod tests {
    use std::collections::BTreeSet;

    use amaru_kernel::{
        Anchor, EraHistory, Hash, Point, PoolId, PoolParams, ProposalId, Slot, StakeCredential,
        TransactionInput, TransactionOutput,
    };
    use proptest::{prelude::Strategy, strategy::ValueTree, test_runner::TestRunner};
    use slot_arithmetic::Epoch;

    use amaru_ledger::{
        state::diff_bind,
        store::{
            columns::{
                accounts::{self},
                cc_members::{self},
                dreps, proposals,
            },
            Columns, ReadOnlyStore, Store, StoreError, TransactionalContext,
        },
    };

    use crate::test_utils::{
        accounts::test::any_stake_credential,
        pools::tests::{any_pool_id, any_pool_params},
        slots::tests::any_slot,
        utxo::test::{any_pseudo_transaction_output, any_txin},
    };

    fn generate_txin() -> TransactionInput {
        any_txin()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_output() -> TransactionOutput {
        any_pseudo_transaction_output()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_stake_credential() -> StakeCredential {
        any_stake_credential()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_account_row() -> accounts::Row {
        crate::test_utils::accounts::test::any_row()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_pool_row() -> (PoolParams, Epoch) {
        let pool_params = any_pool_params()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current();

        let epoch = Epoch::from(0u64);

        (pool_params, epoch)
    }

    fn generate_pool_id() -> PoolId {
        any_pool_id()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_slot() -> Slot {
        any_slot()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_drep_row() -> dreps::Row {
        let mut row = crate::test_utils::dreps::tests::any_row()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current();
        if row.anchor.is_none() {
            row.anchor = Some(dummy_anchor());
        }
        row.previous_deregistration = None;
        row.last_interaction = None;

        row
    }

    fn dummy_anchor() -> Anchor {
        Anchor {
            url: "https://example.com".to_string(),
            content_hash: Hash::from([0u8; 32]),
        }
    }

    fn generate_proposal_id() -> ProposalId {
        crate::test_utils::proposals::tests::any_proposal_id()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_proposal_row() -> amaru_ledger::store::columns::proposals::Row {
        crate::test_utils::proposals::tests::any_row()
            .new_tree(&mut TestRunner::default())
            .unwrap()
            .current()
    }

    fn generate_cc_member_row() -> cc_members::Row {
        let mut runner = TestRunner::default();

        let mut row = crate::test_utils::cc_members::test::any_row()
            .new_tree(&mut runner)
            .unwrap()
            .current();

        if row.hot_credential.is_none() {
            let hot = any_stake_credential()
                .new_tree(&mut runner)
                .unwrap()
                .current();
            row.hot_credential = Some(hot);
        }

        row
    }

    #[derive(Debug, Clone)]
    pub struct Fixture {
        pub txin: TransactionInput,
        pub output: TransactionOutput,
        pub account_key: StakeCredential,
        pub account_row: accounts::Row,
        pub pool_params: PoolParams,
        pub pool_epoch: Epoch,
        pub drep_key: StakeCredential,
        pub drep_row: dreps::Row,
        pub proposal_key: ProposalId,
        pub proposal_row: proposals::Row,
        pub slot: Slot,
        pub slot_leader: PoolId,
        //pub cc_member_key: StakeCredential,
        //pub cc_member_row: cc_members::Row,
    }

    pub fn add_test_data_to_store(
        store: &impl Store,
        era_history: &EraHistory,
    ) -> Result<Fixture, StoreError> {
        use diff_bind::Resettable;

        // utxos
        let txin = generate_txin();
        let output = generate_output();
        let utxos_iter = std::iter::once((txin.clone(), output.clone()));

        // accounts
        let account_key = generate_stake_credential();
        let account_key_clone = account_key.clone();

        let account_row = generate_account_row();

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
        let (pool_params, pool_epoch) = generate_pool_row();
        let pools_iter = std::iter::once((pool_params.clone(), pool_epoch));

        // dreps
        let drep_key = generate_stake_credential();
        let drep_row = generate_drep_row();

        let anchor = drep_row.anchor.clone().expect("Expected anchor to be Some");
        let deposit = drep_row.deposit;
        let registered_at = drep_row.registered_at;

        let drep_epoch = era_history
            .slot_to_epoch(registered_at.transaction.slot)
            .expect("Failed to convert slot to epoch");

        let drep_iter = std::iter::once((
            drep_key.clone(),
            (
                Resettable::Set(anchor),
                Some((deposit, registered_at)),
                drep_epoch,
            ),
        ));

        // proposals
        let proposal_key = generate_proposal_id();
        let proposal_row = generate_proposal_row();
        let proposal_iter = std::iter::once((proposal_key.clone(), proposal_row.clone()));

        // cc_members
        let cc_member_key = generate_stake_credential();
        let cc_member_row = generate_cc_member_row();
        let hot_credential = cc_member_row
            .hot_credential
            .clone()
            .expect("Expected Some hot_credential");
        let cc_members_iter = std::iter::once((
            cc_member_key.clone(),
            Resettable::Set(hot_credential.clone()),
        ));

        let slot = generate_slot();
        let point = Point::Specific(slot.into(), Hash::from([0u8; 32]).to_vec());
        let slot_leader = generate_pool_id();

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
                },
                Columns::empty(),
                std::iter::empty(),
                BTreeSet::new(),
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
            txin,
            output,
            account_key,
            account_row: stored_account_row,
            pool_params,
            pool_epoch,
            drep_key,
            drep_row,
            proposal_key,
            proposal_row,
            slot,
            slot_leader,
        })
    }

    pub fn test_read_utxo(store: &impl ReadOnlyStore, fixture: &Fixture) {
        let result = store
            .utxo(&fixture.txin)
            .expect("failed to read UTXO from store");

        assert_eq!(
            result,
            Some(fixture.output.clone()),
            "UTXO did not match fixture output"
        );
    }

    pub fn test_read_account(store: &impl ReadOnlyStore, fixture: &Fixture) {
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

    pub fn test_read_pool(store: &impl ReadOnlyStore, fixture: &Fixture) {
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

    pub fn test_read_drep(store: &impl ReadOnlyStore, fixture: &Fixture) {
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

    /* Disabled until ReadOnlyStore getter is implemented for cc_members column
    pub fn test_read_cc_member(store: &MemoryStore, fixture: &Fixture) {
        assert_eq!(
            store.cc_member(&fixture.cc_member_key),
            Some(fixture.cc_member_row.clone()),
            "cc_member mismatch"
        );
    }*/

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
        };

        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            BTreeSet::new(),
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
        };

        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            BTreeSet::new(),
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
        };

        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            BTreeSet::new(),
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
        };

        assert!(
            store.iter_dreps()?.any(|(key, _)| key == fixture.drep_key),
            "DRep not present before removal"
        );

        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            BTreeSet::new(),
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

    pub fn test_remove_proposal(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
        let point = Point::Origin;

        let proposal_id = fixture.proposal_key.clone();

        assert!(
            store.iter_proposals()?.any(|(key, _)| key == proposal_id),
            "Proposal not present before removal"
        );

        let remove = Columns {
            utxo: std::iter::empty(),
            pools: std::iter::empty(),
            accounts: std::iter::empty(),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::once(proposal_id.clone()),
        };

        let context = store.create_transaction();
        context.save(
            &point,
            None,
            Columns::empty(),
            remove,
            std::iter::empty(),
            BTreeSet::new(),
        )?;
        context.commit()?;

        let proposal_still_exists = store.iter_proposals()?.any(|(key, _)| key == proposal_id);

        assert!(
            !proposal_still_exists,
            "Proposal was not deleted from store"
        );

        Ok(())
    }

    pub fn test_refund_account(store: &impl Store, fixture: &Fixture) -> Result<(), StoreError> {
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
            let unknown = generate_stake_credential();
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
