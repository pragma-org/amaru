use amaru_kernel::{
    network::NetworkName, protocol_parameters::ProtocolParameters, ComparableProposalId,
    EraHistory, Lovelace, Point, PoolId, ProposalId, Slot, StakeCredential, TransactionInput,
    TransactionOutput,
};
use amaru_ledger::{
    state::diff_bind::Resettable,
    store::{
        columns::{self, accounts, cc_members, dreps, pools, pots, proposals, slots, utxo},
        EpochTransitionProgress, HistoricalStores, ReadOnlyStore, Snapshot, Store, StoreError,
        TransactionalContext,
    },
};

use slot_arithmetic::Epoch;
use std::{
    borrow::{Borrow, BorrowMut},
    cell::{RefCell, RefMut},
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
};

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
pub struct MemoryStore {
    tip: RefCell<Option<Point>>,
    epoch_progress: RefCell<Option<EpochTransitionProgress>>,
    utxos: RefCell<BTreeMap<TransactionInput, utxo::Value>>,
    accounts: RefCell<BTreeMap<StakeCredential, accounts::Row>>,
    pools: RefCell<BTreeMap<PoolId, pools::Row>>,
    pots: RefCell<pots::Row>,
    slots: RefCell<BTreeMap<Slot, slots::Row>>,
    dreps: RefCell<BTreeMap<StakeCredential, dreps::Row>>,
    proposals: RefCell<BTreeMap<ComparableProposalId, proposals::Row>>,
    cc_members: RefCell<BTreeMap<StakeCredential, cc_members::Row>>,
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

impl ReadOnlyStore for MemoryStore {
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
    ) -> Result<Option<amaru_kernel::TransactionOutput>, amaru_ledger::store::StoreError> {
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
            .map(|(proposal_id, row)| (ProposalId::from((*proposal_id).clone()), row.clone()))
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

        // Utxos
        for (key, value) in add.utxo {
            self.store.utxos.borrow_mut().insert(key, value);
        }

        // Pools
        for (pool_params, epoch) in add.pools {
            let mut pools = self.store.pools.borrow_mut();
            let key = pool_params.id;

            let updated_row = match pools.get(&key).cloned() {
                Some(mut row) => {
                    row.future_params.push((Some(pool_params.clone()), epoch));
                    row
                }
                None => columns::pools::Row {
                    current_params: pool_params.clone(),
                    future_params: vec![],
                },
            };

            pools.insert(key, updated_row);
        }

        // Accounts
        for (key, value) in add.accounts {
            let (delegatee, drep, rewards, deposit) = value;

            let mut row =
                self.store
                    .accounts
                    .borrow()
                    .get(&key)
                    .cloned()
                    .unwrap_or(columns::accounts::Row {
                        delegatee: None,
                        drep: None,
                        rewards: 0,
                        deposit,
                    });

            match delegatee {
                Resettable::Set(val) => row.delegatee = Some(val),
                Resettable::Reset => row.delegatee = None,
                Resettable::Unchanged => {}
            }

            match drep {
                Resettable::Set(val) => row.drep = Some(val),
                Resettable::Reset => row.drep = None,
                Resettable::Unchanged => {}
            }

            if let Some(r) = rewards {
                row.rewards = r;
            }

            row.deposit = deposit;

            self.store.accounts.borrow_mut().insert(key, row);
        }

        // Dreps
        let dreps_vec: Vec<_> = add.dreps.collect();

        let newly_registered: BTreeSet<_> = dreps_vec
            .iter()
            .filter_map(|(cred, (_, register, _))| {
                if register.is_some() {
                    Some(cred.clone())
                } else {
                    None
                }
            })
            .collect();

        for (credential, (anchor_resettable, register, _epoch)) in &dreps_vec {
            let mut dreps = self.store.dreps.borrow_mut();
            let existing = dreps.get(credential).cloned();

            if let Some(mut row) = existing {
                if let Some((deposit, registered_at)) = register {
                    row.registered_at = *registered_at;
                    row.deposit = *deposit;
                    row.last_interaction = None;
                }
                anchor_resettable.clone().set_or_reset(&mut row.anchor);
                dreps.insert(credential.clone(), row);
            } else if let Some((deposit, registered_at)) = register {
                let mut row = columns::dreps::Row {
                    anchor: None,
                    deposit: *deposit,
                    registered_at: *registered_at,
                    last_interaction: None,
                    previous_deregistration: None,
                };
                anchor_resettable.clone().set_or_reset(&mut row.anchor);
                dreps.insert(credential.clone(), row);
            } else {
                tracing::error!(
                    target: "store::dreps::add",
                    ?credential,
                    "add.register_no_deposit",
                );
            }
        }

        // Update last_interaction for voting DReps, skipping newly registered ones
        for drep in voting_dreps {
            if newly_registered.contains(&drep) {
                continue;
            }

            let slot = point.slot_or_default();
            let current_epoch = self
                .store
                .era_history
                .slot_to_epoch(slot)
                .map_err(|err| StoreError::Internal(err.into()))?;

            if let Some(drep_row) = self.store.dreps.borrow_mut().get_mut(&drep) {
                drep_row.last_interaction = Some(current_epoch);
            }
        }

        // cc_members
        for (key, value) in add.cc_members {
            match value {
                Resettable::Set(cred) => {
                    let row = columns::cc_members::Row {
                        hot_credential: Some(cred),
                    };
                    self.store.cc_members.borrow_mut().insert(key, row);
                }
                Resettable::Reset => {
                    self.store.cc_members.borrow_mut().remove(&key);
                }
                Resettable::Unchanged => {
                    if let Some(existing) = self.store.cc_members.borrow().get(&key).cloned() {
                        self.store.cc_members.borrow_mut().insert(key, existing);
                    }
                }
            }
        }

        // proposals
        for (proposal_id, value) in add.proposals {
            let key = ComparableProposalId::from(proposal_id);
            self.store.proposals.borrow_mut().insert(key, value);
        }

        // Delete removed data from each respective column in the store
        for key in remove.utxo {
            self.store.utxos.borrow_mut().remove(&key);
        }

        for (pool_id, epoch) in remove.pools {
            let mut pools = self.store.pools.borrow_mut();

            if let Some(row) = pools.get_mut(&pool_id) {
                row.future_params.push((None, epoch));
            }
        }

        for key in remove.accounts {
            self.store.accounts.borrow_mut().remove(&key);
        }

        for (credential, pointer) in remove.dreps {
            let mut dreps = self.store.dreps.borrow_mut();

            if let Some(row) = dreps.get_mut(&credential) {
                row.previous_deregistration = Some(pointer);
            } else {
                tracing::error!(
                    target: "store::dreps::remove",
                    ?credential,
                    "remove.unknown_drep",
                );
            }
        }

        for key in remove.cc_members {
            self.store.cc_members.borrow_mut().remove(&key);
        }

        for key in remove.proposals {
            self.store
                .proposals
                .borrow_mut()
                .remove(&ComparableProposalId::from(key));
        }

        // Reset rewards data for accounts on withdrawal
        for key in withdrawals {
            if let Some(account) = self.store.accounts.borrow_mut().get_mut(&key) {
                account.rewards = 0;
            }
        }

        Ok(())
    }

    fn with_pots(
        &self,
        mut with: impl FnMut(Box<dyn BorrowMut<pots::Row> + 'a>),
    ) -> Result<(), StoreError> {
        with(Box::new(RefMutAdapter::new(self.store.pots.borrow_mut())));

        Ok(())
    }

    fn with_pools(&self, mut with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), StoreError> {
        let mut pools = self.store.pools.borrow_mut();

        let mut owned_pools = BTreeMap::<PoolId, pools::Row>::new();
        std::mem::swap(pools.deref_mut(), &mut owned_pools);

        let mut values: Vec<_> = owned_pools.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = *k;
            let boxed: Box<dyn BorrowMut<Option<pools::Row>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed: BTreeMap<PoolId, pools::Row> = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(pools.deref_mut(), &mut reconstructed);

        Ok(())
    }

    fn with_accounts(
        &self,
        mut with: impl FnMut(accounts::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let mut accounts = self.store.accounts.borrow_mut();

        let mut owned_accounts = BTreeMap::<StakeCredential, accounts::Row>::new();
        std::mem::swap(accounts.deref_mut(), &mut owned_accounts);

        let mut values: Vec<_> = owned_accounts
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = k.clone();
            let boxed: Box<dyn BorrowMut<Option<accounts::Row>>> =
                Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(accounts.deref_mut(), &mut reconstructed);

        Ok(())
    }

    fn with_block_issuers(
        &self,
        mut with: impl FnMut(amaru_ledger::store::columns::slots::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let mut slots = self.store.slots.borrow_mut();

        let mut owned_slots = BTreeMap::<Slot, slots::Row>::new();
        std::mem::swap(slots.deref_mut(), &mut owned_slots);

        let mut values: Vec<_> = owned_slots.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = *k;
            let boxed: Box<dyn BorrowMut<Option<slots::Row>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed: BTreeMap<Slot, slots::Row> = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(slots.deref_mut(), &mut reconstructed);

        Ok(())
    }

    fn with_utxo(
        &self,
        mut with: impl FnMut(amaru_ledger::store::columns::utxo::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let mut utxos = self.store.utxos.borrow_mut();

        let mut owned_utxos = BTreeMap::<TransactionInput, TransactionOutput>::new();
        std::mem::swap(utxos.deref_mut(), &mut owned_utxos);

        let mut values: Vec<_> = owned_utxos.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = k.clone();
            let boxed: Box<dyn BorrowMut<Option<utxo::Value>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed: BTreeMap<TransactionInput, TransactionOutput> = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(utxos.deref_mut(), &mut reconstructed);
        Ok(())
    }

    fn with_dreps(
        &self,
        mut with: impl FnMut(amaru_ledger::store::columns::dreps::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let mut dreps = self.store.dreps.borrow_mut();

        let mut owned_dreps = BTreeMap::<StakeCredential, dreps::Row>::new();
        std::mem::swap(dreps.deref_mut(), &mut owned_dreps);

        let mut values: Vec<_> = owned_dreps.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = k.clone();
            let boxed: Box<dyn BorrowMut<Option<dreps::Row>>> = Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed: BTreeMap<StakeCredential, dreps::Row> = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(dreps.deref_mut(), &mut reconstructed);

        Ok(())
    }

    fn with_proposals(
        &self,
        mut with: impl FnMut(amaru_ledger::store::columns::proposals::Iter<'_, '_>),
    ) -> Result<(), amaru_ledger::store::StoreError> {
        let mut proposals = self.store.proposals.borrow_mut();

        let mut owned = BTreeMap::<ComparableProposalId, proposals::Row>::new();
        std::mem::swap(proposals.deref_mut(), &mut owned);

        let mut values: Vec<_> = owned.into_iter().map(|(k, v)| (k, Some(v))).collect();

        let iter = values.iter_mut().map(|(k, v)| {
            let key = ProposalId::from(k.clone());
            let boxed: Box<dyn BorrowMut<Option<proposals::Row>>> =
                Box::new(RefMutAdapterMut::new(v));
            (key, boxed)
        });

        with(Box::new(iter));

        let mut reconstructed: BTreeMap<ComparableProposalId, proposals::Row> = values
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        std::mem::swap(proposals.deref_mut(), &mut reconstructed);

        Ok(())
    }
}

impl Store for MemoryStore {
    type Transaction<'a> = MemoryTransactionalContext<'a>;

    // TODO: Implement snapshots on MemoryStore (Placeholder used to pass bootstrap all stages test)
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        Ok(vec![Epoch::from(3)])
    }

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
    fn for_epoch(&self, _epoch: Epoch) -> Result<impl Snapshot, amaru_ledger::store::StoreError> {
        let era_history: &EraHistory = NetworkName::Preprod.into();
        Ok(MemoryStore::new(era_history.clone()))
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::network::NetworkName;
    use amaru_kernel::EraHistory;

    use crate::in_memory::MemoryStore;
    use crate::tests::{
        add_test_data_to_store, test_epoch_transition, test_read_account, test_read_drep,
        test_read_pool, test_read_proposal, test_read_utxo, test_refund_account,
        test_remove_account, test_remove_drep, test_remove_pool, test_remove_proposal,
        test_remove_utxo, test_slot_updated,
    };
    use amaru_ledger::store::StoreError;

    #[test]
    fn test_in_memory_store() -> Result<(), StoreError> {
        let era_history: EraHistory =
            (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let store = MemoryStore::new(era_history.clone());

        // Add to store test
        let seeded =
            add_test_data_to_store(&store, &era_history).expect("adding data to store failed");

        // Validate add to store & read tests
        test_read_utxo(&store, &seeded);
        test_read_account(&store, &seeded);
        test_read_pool(&store, &seeded);
        test_read_drep(&store, &seeded);
        test_read_proposal(&store, &seeded);
        // TODO: Add cc_members iterator to validate getting stored cc_member works as intended

        // Transactional tests
        test_refund_account(&store, &seeded)?;
        test_epoch_transition(&store)?;
        test_slot_updated(&store, &seeded)?;

        // Validate removal tests
        test_remove_utxo(&store, &seeded)?;
        test_remove_account(&store, &seeded)?;
        test_remove_pool(&store, &seeded)?;
        test_remove_drep(&store, &seeded)?;
        test_remove_proposal(&store, &seeded)?;
        // TODO: Add cc_members iterator to validate removal works as intended

        Ok(())
    }
}
