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

use std::collections::BTreeSet;

use amaru_kernel::{ComparableProposalId, DRep, PoolId, ProposalId, RewardAccount, StakeCredential, TransactionInput};

use crate::context::{
    PreparationContext, PrepareAccountsSlice, PrepareCommitteeSlice, PrepareDRepsSlice, PreparePoolsSlice,
    PrepareProposalsSlice, PrepareUtxoSlice,
};

/// An implementation of the block preparation context that's suitable for use in normal operation.
///
/// It is for now incomplete, but we'll use eventually bridge the gap between the validations and
/// the state management so that this fully replaces the current state module and child modules.
///
/// For now, there's still a bit of duplication between the modules.
#[derive(Debug, Default)]
pub struct DefaultPreparationContext<'a> {
    pub utxo: BTreeSet<&'a TransactionInput>,
    pub pools: BTreeSet<&'a PoolId>,
    pub accounts: BTreeSet<&'a StakeCredential>,
    pub withdrawals: BTreeSet<&'a RewardAccount>,
    pub dreps: BTreeSet<&'a StakeCredential>,
    pub drep_delegations: BTreeSet<&'a DRep>,
    pub committee: BTreeSet<&'a StakeCredential>,
    pub proposals: BTreeSet<ComparableProposalId>,
}

impl DefaultPreparationContext<'_> {
    pub fn new() -> Self {
        Self {
            utxo: BTreeSet::new(),
            pools: BTreeSet::new(),
            accounts: BTreeSet::new(),
            withdrawals: BTreeSet::new(),
            dreps: BTreeSet::new(),
            drep_delegations: BTreeSet::new(),
            committee: BTreeSet::new(),
            proposals: BTreeSet::new(),
        }
    }
}

impl<'a> PreparationContext<'a> for DefaultPreparationContext<'a> {}

impl<'a> PrepareUtxoSlice<'a> for DefaultPreparationContext<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput) {
        self.utxo.insert(input);
    }
}

impl<'a> PreparePoolsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_pool(&mut self, pool: &'a PoolId) {
        self.pools.insert(pool);
    }
}

impl<'a> PrepareAccountsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_account(&mut self, credential: &'a StakeCredential) {
        self.accounts.insert(credential);
    }

    fn require_withdrawal(&mut self, reward_account: &'a RewardAccount) {
        self.withdrawals.insert(reward_account);
    }
}

impl<'a> PrepareDRepsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_drep(&mut self, drep: &'a StakeCredential) {
        self.dreps.insert(drep);
    }

    fn require_drep_delegation(&mut self, drep: &'a DRep) {
        self.drep_delegations.insert(drep);
    }
}

impl<'a> PrepareCommitteeSlice<'a> for DefaultPreparationContext<'a> {
    fn require_committee_member(&mut self, cc_member: &'a StakeCredential) {
        self.committee.insert(cc_member);
    }
}

impl<'a> PrepareProposalsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_proposal(&mut self, id: &'a ProposalId) {
        self.proposals.insert(ComparableProposalId::from(id.clone()));
    }
}
