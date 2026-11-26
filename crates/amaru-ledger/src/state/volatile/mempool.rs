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

use std::collections::BTreeMap;

use amaru_kernel::{Credential, Lovelace, MemoizedTransactionOutput, PoolId, Pots, ProposalId, TransactionInput};

use super::{
    AccountBind, Bind, CommitteeMemberBind, DRepBind, Existence, RewardsAtTip, VolatileAggregate, VolatileDB,
    VolatileFragment, VolatileState,
};
use crate::context::ProposalStateSlim;

/// A transaction-validation view made by layering already-validated mempool fragments over the
/// ledger's volatile state.
///
/// The view is intentionally short-lived. It owns the accumulated transaction changes while
/// borrowing the ledger state below them, so a transaction can see outputs and registrations from
/// earlier mempool transactions without changing the ledger before a block is forged.
pub struct VolatileMempool<'a> {
    base: &'a VolatileDB,
    aggregate: VolatileAggregate,
}

impl<'a> VolatileMempool<'a> {
    pub fn new(base: &'a VolatileDB, fragments: impl IntoIterator<Item = &'a VolatileFragment>) -> Self {
        let mut aggregate = VolatileAggregate::default();
        for fragment in fragments {
            aggregate.add_fragment(fragment);
        }
        Self { base, aggregate }
    }
}

impl VolatileState for VolatileMempool<'_> {
    type TransactionOutput<'a>
        = Existence<&'a MemoizedTransactionOutput>
    where
        Self: 'a;
    fn resolve_input<'a>(&'a self, input: &TransactionInput) -> Self::TransactionOutput<'a> {
        self.aggregate.resolve_input(input).or_else(|| self.base.resolve_input(input))
    }

    type Pool = Existence<()>;
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool {
        if self.aggregate.resolve_pool(pool_id) { Existence::Exists(()) } else { self.base.resolve_pool(pool_id) }
    }

    type Account<'a>
        = (Existence<AccountBind<'a>>, RewardsAtTip)
    where
        Self: 'a;
    fn resolve_account<'a>(&'a self, credential: &Credential) -> Self::Account<'a> {
        let (base_account, rewards) = self.base.resolve_account(credential);
        let account = self.aggregate.resolve_account(credential).chain(|| base_account);
        let rewards = if self.aggregate.has_withdrawal(credential) { RewardsAtTip::Reset } else { rewards };
        (account, rewards)
    }

    fn has_withdrawal(&self, credential: &Credential) -> bool {
        self.aggregate.has_withdrawal(credential) || self.base.has_withdrawal(credential)
    }

    type DRep<'a>
        = Existence<DRepBind<'a>>
    where
        Self: 'a;
    fn resolve_drep<'a>(&'a self, credential: &Credential) -> Self::DRep<'a> {
        self.aggregate.resolve_drep(credential).chain(|| self.base.resolve_drep(credential))
    }

    type CCMembers<'a>
        = BTreeMap<&'a Credential, Existence<CommitteeMemberBind<'a>>>
    where
        Self: 'a;
    fn resolve_cc_members<'a>(&'a self) -> Self::CCMembers<'a> {
        let mut members = self.base.resolve_cc_members();
        let updates = self.aggregate.resolve_cc_members().chain(
            self.aggregate
                .resolve_committee_candidates()
                .map(|(_, credential)| (credential, Existence::Exists(Bind::default()))),
        );

        for (credential, newer) in updates {
            let older = members.remove(credential).unwrap_or(Existence::Unknown);
            members.insert(credential, Existence::fold([newer, older].into_iter()));
        }
        members
    }

    type Proposal = Existence<ProposalStateSlim>;
    fn resolve_proposal(&self, id: &ProposalId) -> Self::Proposal {
        self.aggregate.resolve_proposal(id).or_else(|| self.base.resolve_proposal(id))
    }

    fn resolve_treasury(&self, pots: &Pots) -> Lovelace {
        self.base.resolve_treasury(pots) + self.aggregate.donations()
    }

    fn resolve_donations(&self) -> Lovelace {
        self.base.resolve_donations() + self.aggregate.donations()
    }
}
