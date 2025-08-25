// Copyright 2025 PRAGMA
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
    ComparableProposalId, Constitution, ConstitutionalCommittee, Point, PoolId, Slot,
    StakeCredential, TransactionInput, protocol_parameters::ProtocolParameters,
};
use amaru_ledger::{
    governance::ratification::ProposalsRoots,
    store::{
        GovernanceActivity, ReadStore, Snapshot, StoreError,
        columns::{
            accounts as accounts_column, cc_members as cc_members_column, dreps as dreps_column,
            pools as pools_column, pots, proposals as proposals_column, slots, utxo as utxo_column,
            votes as votes_column,
        },
    },
};
use amaru_slot_arithmetic::Epoch;
use std::collections::BTreeMap;

use crate::in_memory::MemoryStore;

#[derive(Clone, Debug)]
pub struct MemorySnapshot {
    pub epoch: Epoch,
    pub tip: Point,
    pub constitution: Option<Constitution>,
    pub proposals_roots: Option<ProposalsRoots>,
    pub governance_activity: Option<GovernanceActivity>,
    pub protocol_parameters: Option<ProtocolParameters>,
    pub constitutional_committee: ConstitutionalCommittee,

    // TODO: Switch large states to use OrdMap
    pub utxos: BTreeMap<TransactionInput, utxo_column::Value>,
    pub accounts: BTreeMap<StakeCredential, accounts_column::Row>,
    pub pools: BTreeMap<PoolId, pools_column::Row>,
    pub pots: pots::Row,
    pub slots: BTreeMap<Slot, slots::Row>,
    pub dreps: BTreeMap<StakeCredential, dreps_column::Row>,
    pub proposals: BTreeMap<ComparableProposalId, proposals_column::Row>,
    pub cc_members: BTreeMap<StakeCredential, cc_members_column::Row>,
    pub votes: BTreeMap<votes_column::Key, votes_column::Value>,
}

impl MemorySnapshot {
    pub fn new(epoch: Epoch, store: &MemoryStore) -> Self {
        Self {
            epoch,
            tip: store.tip.borrow().clone(),
            constitution: store.constitution.borrow().clone(),
            proposals_roots: store.proposals_roots.borrow().clone(),
            governance_activity: store.governance_activity.borrow().clone(),
            protocol_parameters: store.protocol_parameters.borrow().clone(),
            constitutional_committee: store.constitutional_committee.borrow().clone(),

            utxos: store.utxos.borrow().clone(),
            accounts: store.accounts.borrow().clone(),
            pools: store.pools.borrow().clone(),
            pots: store.pots.borrow().clone(),
            slots: store.slots.borrow().clone(),

            dreps: store.dreps.borrow().clone(),
            proposals: store.proposals.borrow().clone(),
            cc_members: store.cc_members.borrow().clone(),
            votes: store.votes.borrow().clone(),
        }
    }
}

impl Snapshot for MemorySnapshot {
    fn epoch(&self) -> Epoch {
        self.epoch
    }
}

impl ReadStore for MemorySnapshot {
    fn tip(&self) -> Result<Point, StoreError> {
        Ok(self.tip.clone())
    }

    fn protocol_parameters(&self) -> Result<ProtocolParameters, StoreError> {
        self.protocol_parameters
            .clone()
            .ok_or(StoreError::missing::<ProtocolParameters>(
                "protocol_parameters",
            ))
    }

    fn proposals_roots(&self) -> Result<ProposalsRoots, StoreError> {
        self.proposals_roots
            .clone()
            .ok_or(StoreError::missing::<ProposalsRoots>("proposals_roots"))
    }

    fn constitutional_committee(&self) -> Result<ConstitutionalCommittee, StoreError> {
        Ok(self.constitutional_committee.clone())
    }

    fn constitution(&self) -> Result<Constitution, StoreError> {
        self.constitution
            .clone()
            .ok_or(StoreError::missing::<Constitution>("constitution"))
    }

    fn governance_activity(&self) -> Result<GovernanceActivity, StoreError> {
        self.governance_activity
            .clone()
            .ok_or(StoreError::missing::<GovernanceActivity>(
                "governance_activity",
            ))
    }

    fn account(
        &self,
        credential: &StakeCredential,
    ) -> Result<Option<accounts_column::Row>, StoreError> {
        Ok(self.accounts.get(credential).cloned())
    }

    fn pool(&self, pool: &PoolId) -> Result<Option<pools_column::Row>, StoreError> {
        Ok(self.pools.get(pool).cloned())
    }

    fn utxo(&self, input: &TransactionInput) -> Result<Option<utxo_column::Value>, StoreError> {
        Ok(self.utxos.get(input).cloned())
    }

    fn pots(&self) -> Result<amaru_ledger::summary::Pots, StoreError> {
        Ok((&self.pots).into())
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
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
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
        let slots_vec: Vec<_> = self.slots.iter().map(|(k, v)| (*k, v.clone())).collect();
        Ok(slots_vec.into_iter())
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
        let pools_vec: Vec<_> = self.pools.iter().map(|(k, v)| (*k, v.clone())).collect();
        Ok(pools_vec.into_iter())
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
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
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
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
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
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(proposals_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_cc_members(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::cc_members::Key,
                amaru_ledger::store::columns::cc_members::Row,
            ),
        >,
        StoreError,
    > {
        let cc_vec: Vec<_> = self
            .cc_members
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(cc_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_votes(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                amaru_ledger::store::columns::votes::Key,
                amaru_ledger::store::columns::votes::Row,
            ),
        >,
        StoreError,
    > {
        let votes_vec: Vec<_> = self
            .votes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(votes_vec.into_iter())
    }
}
