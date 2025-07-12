use amaru_kernel::{
    protocol_parameters::ProtocolParameters, ComparableProposalId, EraHistory, Lovelace, Point,
    PoolId, ProposalId, Slot, StakeCredential, TransactionInput,
};
use amaru_ledger::store::{
    columns::{
        accounts as account_column, cc_members as cc_member_column, dreps as drep_column,
        pools as pool_column, pots, proposals as proposal_column, slots, utxo as utxo_column,
    },
    EpochTransitionProgress, HistoricalStores, ReadStore, Snapshot, Store, StoreError,
    TransactionalContext,
};

use iter_borrow::IterBorrow;
use slot_arithmetic::Epoch;
use std::{
    borrow::{Borrow, BorrowMut},
    cell::{RefCell, RefMut},
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
};

pub mod ledger;
use crate::in_memory::ledger::columns::{accounts, cc_members, dreps, pools, proposals, utxo};

pub struct RefMutAdapter<'a, T> {
    inner: RefMut<'a, T>,
}

impl<'a, T> RefMutAdapter<'a, T> {
    pub fn new(inner: RefMut<'a, T>) -> Self {
        Self { inner }
    }
}

impl<'a, T> Borrow<T> for RefMutAdapter<'a, T> {
    fn borrow(&self) -> &T {
        self.inner.deref()
    }
}

impl<'a, T> BorrowMut<T> for RefMutAdapter<'a, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

pub struct RefMutAdapterMut<'a, T> {
    inner: &'a mut T,
}

impl<'a, T> RefMutAdapterMut<'a, T> {
    pub fn new(inner: &'a mut T) -> Self {
        Self { inner }
    }
}

impl<'a, T> Borrow<T> for RefMutAdapterMut<'a, T> {
    fn borrow(&self) -> &T {
        self.inner
    }
}

impl<'a, T> BorrowMut<T> for RefMutAdapterMut<'a, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner
    }
}

// TODO: Add a field to MemoryStore for storing per-epoch snapshots as nested MemoryStores
#[derive(Clone)]
pub struct MemoryStore {
    tip: RefCell<Option<Point>>,
    epoch_progress: RefCell<Option<EpochTransitionProgress>>,
    utxos: RefCell<BTreeMap<TransactionInput, utxo_column::Value>>,
    accounts: RefCell<BTreeMap<StakeCredential, account_column::Row>>,
    pools: RefCell<BTreeMap<PoolId, pool_column::Row>>,
    pots: RefCell<pots::Row>,
    slots: RefCell<BTreeMap<Slot, slots::Row>>,
    dreps: RefCell<BTreeMap<StakeCredential, drep_column::Row>>,
    proposals: RefCell<BTreeMap<ComparableProposalId, proposal_column::Row>>,
    cc_members: RefCell<BTreeMap<StakeCredential, cc_member_column::Row>>,
    p_params: RefCell<BTreeMap<Epoch, ProtocolParameters>>,
    era_history: EraHistory,
}

impl MemoryStore {
    pub fn new(era_history: EraHistory) -> Self {
        MemoryStore {
            tip: RefCell::new(Some(Point::Origin)),
            epoch_progress: RefCell::new(None),
            utxos: RefCell::new(BTreeMap::new()),
            accounts: RefCell::new(BTreeMap::new()),
            pools: RefCell::new(BTreeMap::new()),
            pots: RefCell::new(pots::Row::default()),
            slots: RefCell::new(BTreeMap::new()),
            dreps: RefCell::new(BTreeMap::new()),
            proposals: RefCell::new(BTreeMap::new()),
            cc_members: RefCell::new(BTreeMap::new()),
            p_params: RefCell::new(BTreeMap::new()),
            era_history,
        }
    }
}

// TODO: Implement Snapshot on MemoryStore (Currently returns hard-coded epoch 10)
impl Snapshot for MemoryStore {
    fn epoch(&self) -> Epoch {
        Epoch::from(10)
    }
}

impl ReadStore for MemoryStore {
    fn get_protocol_parameters_for(&self, epoch: &Epoch) -> Result<ProtocolParameters, StoreError> {
        let map = self.p_params.borrow();
        let params = map.get(epoch).cloned().unwrap_or_default();
        Ok(params)
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> Result<Option<amaru_ledger::store::columns::accounts::Row>, amaru_ledger::store::StoreError>
    {
        Ok(self.accounts.borrow().get(credential).cloned())
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> Result<Option<amaru_ledger::store::columns::pools::Row>, amaru_ledger::store::StoreError>
    {
        Ok(self.pools.borrow().get(pool).cloned())
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> Result<Option<amaru_kernel::MemoizedTransactionOutput>, amaru_ledger::store::StoreError>
    {
        Ok(self.utxos.borrow().get(input).cloned())
    }

    fn pots(&self) -> Result<amaru_ledger::summary::Pots, amaru_ledger::store::StoreError> {
        Ok((&*self.pots.borrow()).into())
    }

    #[allow(refining_impl_trait)]
    fn iter_utxos(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::utxo::Key,
                amaru_ledger::store::columns::utxo::Value,
            ),
        >,
        amaru_ledger::store::StoreError,
    > {
        let utxo_vec: Vec<_> = self
            .utxos
            .borrow()
            .iter()
            .map(|(tx_input, tx_output)| (tx_input.clone(), tx_output.clone()))
            .collect();
        Ok(utxo_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_block_issuers(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::slots::Key,
                amaru_ledger::store::columns::slots::Value,
            ),
        >,
        StoreError,
    > {
        let block_issuer_vec: Vec<_> = self
            .slots
            .borrow()
            .iter()
            .map(|(slot, row)| (*slot, row.clone()))
            .collect();

        Ok(block_issuer_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_pools(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::pools::Key,
                amaru_ledger::store::columns::pools::Row,
            ),
        >,
        StoreError,
    > {
        let pool_vec: Vec<_> = self
            .pools
            .borrow()
            .iter()
            .map(|(pool_id, row)| (*pool_id, row.clone()))
            .collect();

        Ok(pool_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_accounts(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::accounts::Key,
                amaru_ledger::store::columns::accounts::Row,
            ),
        >,
        StoreError,
    > {
        let accounts_vec: Vec<_> = self
            .accounts
            .borrow()
            .iter()
            .map(|(stake_credential, row)| (stake_credential.clone(), row.clone()))
            .collect();

        Ok(accounts_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_dreps(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::dreps::Key,
                amaru_ledger::store::columns::dreps::Row,
            ),
        >,
        StoreError,
    > {
        let dreps_vec: Vec<_> = self
            .dreps
            .borrow()
            .iter()
            .map(|(stake_credential, row)| (stake_credential.clone(), row.clone()))
            .collect();

        Ok(dreps_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_proposals(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::proposals::Key,
                amaru_ledger::store::columns::proposals::Row,
            ),
        >,
        StoreError,
    > {
        let proposals_vec: Vec<_> = self
            .proposals
            .borrow()
            .iter()
            .map(|(proposal_id, row)| (ProposalId::from(proposal_id.clone()), row.clone()))
            .collect();

        Ok(proposals_vec.into_iter())
    }
}

pub struct MemoryTransactionalContext<'a> {
    store: &'a MemoryStore,
}

impl<'a> MemoryTransactionalContext<'a> {
    pub fn new(store: &'a MemoryStore) -> Self {
        Self { store }
    }
}

impl<'a> MemoryTransactionalContext<'a> {
    pub fn with_column<K, V, F>(
        &self,
        column: &RefCell<BTreeMap<K, V>>,
        mut with: F,
    ) -> Result<(), StoreError>
    where
        K: Clone + Ord + 'a,
        V: Clone + 'a,
        F: for<'iter> FnMut(IterBorrow<'iter, 'iter, K, Option<V>>),
    {
        let original = column.take();
        let mut values: Vec<(K, Option<V>)> =
            original.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = k.clone();
            let boxed: Box<dyn BorrowMut<Option<V>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let rebuilt: BTreeMap<K, V> = values
            .into_iter()
            .flat_map(|(k, v)| v.map(|v| (k, v)))
            .collect();

        column.replace(rebuilt);
        Ok(())
    }
}

impl<'a> TransactionalContext<'a> for MemoryTransactionalContext<'a> {
    fn commit(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn rollback(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool, StoreError> {
        let mut progress = self.store.epoch_progress.borrow_mut();

        if *progress != from {
            return Ok(false);
        }
        *progress = to;
        Ok(true)
    }

    fn refund(
        &self,
        credential: &amaru_ledger::store::columns::accounts::Key,
        deposit: Lovelace,
    ) -> Result<Lovelace, StoreError> {
        let mut accounts = self.store.accounts.borrow_mut();
        match accounts.get_mut(credential) {
            Some(account) => {
                account.rewards += deposit;
                Ok(0)
            }
            _ => Ok(deposit),
        }
    }

    fn set_protocol_parameters(
        &self,
        epoch: &Epoch,
        protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError> {
        self.store
            .p_params
            .borrow_mut()
            .insert(*epoch, protocol_parameters.clone());
        Ok(())
    }

    fn save(
        &self,
        point: &Point,
        issuer: Option<&amaru_ledger::store::columns::pools::Key>,
        add: amaru_ledger::store::Columns<
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::utxo::Key,
                    amaru_ledger::store::columns::utxo::Value,
                ),
            >,
            impl Iterator<Item = amaru_ledger::store::columns::pools::Value>,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::accounts::Key,
                    amaru_ledger::store::columns::accounts::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::dreps::Key,
                    amaru_ledger::store::columns::dreps::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::cc_members::Key,
                    amaru_ledger::store::columns::cc_members::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::proposals::Key,
                    amaru_ledger::store::columns::proposals::Value,
                ),
            >,
        >,
        remove: amaru_ledger::store::Columns<
            impl Iterator<Item = amaru_ledger::store::columns::utxo::Key>,
            impl Iterator<Item = (amaru_ledger::store::columns::pools::Key, Epoch)>,
            impl Iterator<Item = amaru_ledger::store::columns::accounts::Key>,
            impl Iterator<
                Item = (
                    amaru_ledger::store::columns::dreps::Key,
                    amaru_kernel::CertificatePointer,
                ),
            >,
            impl Iterator<Item = amaru_ledger::store::columns::cc_members::Key>,
            impl Iterator<Item = amaru_ledger::store::columns::proposals::Key>,
        >,
        withdrawals: impl Iterator<Item = amaru_ledger::store::columns::accounts::Key>,
        voting_dreps: BTreeSet<StakeCredential>,
        _era_history: &EraHistory,
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let current_tip = self.store.tip.borrow().clone();

        match (point, current_tip) {
            (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                tracing::trace!(target: "event.store.memory", ?point, "save.point_already_known");
                return Ok(());
            }
            _ => {
                // Update tip
                *self.store.tip.borrow_mut() = Some(point.clone());

                // Add issuer to new slot row
                if let Point::Specific(slot, _) = point {
                    if let Some(issuer) = issuer {
                        self.store.slots.borrow_mut().insert(
                            Slot::from(*slot),
                            amaru_ledger::store::columns::slots::Row::new(*issuer),
                        );
                    }
                }
            }
        }

        utxo::add(self.store, add.utxo)?;
        pools::add(self.store, add.pools);
        dreps::add(self.store, add.dreps)?;
        accounts::add(self.store, add.accounts)?;
        cc_members::add(self.store, add.cc_members)?;
        proposals::add(self.store, add.proposals)?;

        accounts::reset_many(self.store, withdrawals)?;
        dreps::tick(self.store, &voting_dreps, point)?;

        utxo::remove(self.store, remove.utxo)?;
        pools::remove(self.store, remove.pools)?;
        accounts::remove(self.store, remove.accounts)?;
        dreps::remove(self.store, remove.dreps)?;
        cc_members::remove(self.store, remove.cc_members)?;
        proposals::remove(self.store, remove.proposals)?;
        Ok(())
    }

    fn with_pots(
        &self,
        mut with: impl FnMut(Box<dyn BorrowMut<pots::Row> + 'a>),
    ) -> Result<(), StoreError> {
        with(Box::new(RefMutAdapter::new(self.store.pots.borrow_mut())));

        Ok(())
    }

    fn with_pools(&self, with: impl FnMut(pool_column::Iter<'_, '_>)) -> Result<(), StoreError> {
        self.with_column(&self.store.pools, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(account_column::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.accounts, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::slots::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.slots, with)
    }

    fn with_utxo(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::utxo::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.utxos, with)
    }

    fn with_dreps(
        &self,
        with: impl FnMut(amaru_ledger::store::columns::dreps::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        self.with_column(&self.store.dreps, with)
    }

    fn with_proposals(
        &self,
        mut with: impl FnMut(amaru_ledger::store::columns::proposals::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        self.with_column(&self.store.proposals, |iter| {
            let mapped = iter.map(|(k, v)| (ProposalId::from(k), v));
            with(Box::new(mapped));
        })
    }
}

impl Store for MemoryStore {
    type Transaction<'a> = MemoryTransactionalContext<'a>;

    // TODO: Implement next_snapshot on MemoryStore
    fn next_snapshot(&self, _epoch: Epoch) -> Result<(), StoreError> {
        Err(StoreError::Internal(
            "next_snapshot not yet implemented on MemoryStore".into(),
        ))
    }

    fn create_transaction(&self) -> Self::Transaction<'_> {
        MemoryTransactionalContext { store: self }
    }

    fn tip(&self) -> Result<Point, StoreError> {
        self.tip
            .borrow()
            .clone()
            .ok_or_else(|| StoreError::Internal("tip not yet set".into()))
    }
}

// TODO: Implement HistoricalStores on MemoryStore (currently returns a new empty MemoryStore)
impl HistoricalStores for MemoryStore {
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        Ok(vec![Epoch::from(3)])
    }

    fn for_epoch(&self, _epoch: Epoch) -> Result<impl Snapshot, amaru_ledger::store::StoreError> {
        Ok(MemoryStore::new(self.era_history.clone()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        in_memory::MemoryStore,
        tests::{
            add_test_data_to_store, test_epoch_transition, test_read_account, test_read_drep,
            test_read_pool, test_read_proposal, test_refund_account, test_remove_account,
            test_remove_drep, test_remove_pool, test_remove_proposal, test_slot_updated, Fixture,
        },
    };
    use amaru_kernel::{network::NetworkName, EraHistory};
    use amaru_ledger::store::StoreError;
    use proptest::test_runner::TestRunner;

    pub fn setup_memory_store(
        runner: &mut TestRunner,
    ) -> Result<(MemoryStore, Fixture), StoreError> {
        let era_history: &EraHistory = NetworkName::Preprod.into();
        let store = MemoryStore::new(era_history.clone());
        let fixture = add_test_data_to_store(&store, era_history, runner)?;
        Ok((store, fixture))
    }

    #[test]
    fn test_in_mem_read_account() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_account(&store, &fixture);
    }

    #[test]
    fn test_in_mem_read_pool() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_pool(&store, &fixture);
    }

    #[test]
    fn test_in_mem_read_drep() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_drep(&store, &fixture);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_in_mem_read_proposal() {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_read_proposal(&store, &fixture);
    }

    #[test]
    fn test_in_mem_refund_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_refund_account(&store, &fixture, &mut runner)
    }

    #[test]
    fn test_in_mem_epoch_transition() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, _) = setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_epoch_transition(&store)
    }

    #[test]
    fn test_in_mem_slot_updated() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_slot_updated(&store, &fixture)
    }

    /* Disabled until MemoizedTransactionOutput is created
        #[test]
        fn test_in_mem_read_utxo() {
            let mut runner = TestRunner::default();
            let (store, fixture) =
                setup_memory_store(&mut runner).expect("Failed to add test data to store");
            test_read_utxo(&store, &fixture);
        }

        #[test]
        fn test_in_mem_remove_utxo() -> Result<(), StoreError> {
            let mut runner = TestRunner::default();
            let (store, fixture) =
                setup_memory_store(&mut runner).expect("Failed to add test data to store");
            test_remove_utxo(&store, &fixture)
        }
    */

    #[test]
    fn test_in_mem_remove_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_account(&store, &fixture)
    }

    #[test]
    fn test_in_mem_remove_pool() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_pool(&store, &fixture)
    }

    #[test]
    fn test_in_mem_remove_drep() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_drep(&store, &fixture)
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_in_mem_remove_proposal() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) =
            setup_memory_store(&mut runner).expect("Failed to add test data to store");
        test_remove_proposal(&store, &fixture)
    }

    #[test]
    #[ignore]
    fn test_in_mem_iterate_cc_members() {
        unimplemented!()
    }

    #[test]
    #[ignore]
    fn test_in_mem_remove_cc_members() {
        unimplemented!()
    }
}
