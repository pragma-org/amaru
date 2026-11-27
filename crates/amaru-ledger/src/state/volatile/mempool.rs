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
    // TODO: Allow rebuilding mempool view incrementally
    //
    // We build new VolatileMempool each time we need to validate new incoming transactions. We also
    // use that same model when re-validating the mempool, where the volatile db is fixed, and the
    // fragments are stacked on top of each other. That means we end up re-building the same base +
    // fragment sequence for each transaction we re-validate, instead of simply applying the next
    // fragment to the previous aggregate.
    //
    // This is only "okay" because our mempool is small, but it is a somewhat quadratic operation
    // which we should definitely avoid in the final mempool design.
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
        // NOTE: Pending treasury values in mempool
        //
        // Transactions in mempool cannot influence the treasury. The value only changes at epoch
        // boundary, and is only affected by epoch overlays. This is why here we can simply fallback
        // to the base value.
        self.base.resolve_treasury(pots)
    }

    fn resolve_donations(&self) -> Lovelace {
        self.base.resolve_donations() + self.aggregate.donations()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{
        CertificatePointer, Credential, Epoch, GovernanceAction, Hash, PoolId, Proposal, ProposalId, ProposalPointer,
        Set, Slot, TransactionPointer, UnitRationalNumber, utils::tests::run_strategy,
    };
    use proptest::prelude::any;

    use super::*;
    use crate::{
        context::ProposalState,
        state::volatile::{AnchoredVolatileFragment, VolatileSequence},
    };

    #[test]
    fn accounts_match_when_mempool_fragments_are_appended_to_current() {
        let credential = credential(1);
        let mut base_fragment = VolatileFragment::default();
        base_fragment.accounts.register(credential, 2_000_000, None, None).unwrap();

        let mut binding = VolatileFragment::default();
        binding
            .accounts
            .bind_left(
                credential,
                Some((
                    PoolId::new([2; 28]),
                    CertificatePointer {
                        transaction: TransactionPointer { slot: Slot::from(20), transaction_index: 0 },
                        certificate_index: 0,
                    },
                )),
            )
            .unwrap();

        let mut withdrawal = VolatileFragment::default();
        withdrawal.withdrawals.insert(credential);

        let fragments = [binding, withdrawal];
        let (base, mut appended) = databases_with_base(&base_fragment);
        let view = VolatileMempool::new(&base, fragments.iter());
        append_fragments(&mut appended, &fragments);

        assert_eq!(view.resolve_account(&credential), appended.resolve_account(&credential));
        assert_eq!(view.has_withdrawal(&credential), appended.has_withdrawal(&credential));
    }

    #[test]
    fn committee_members_match_when_mempool_fragments_are_appended_to_current() {
        let member = credential(1);
        let mut base_fragment = VolatileFragment::default();
        base_fragment.committee.bind_left(member, Some(credential(2).into())).unwrap();

        let mut resignation = VolatileFragment::default();
        resignation.committee.bind_left(member, None).unwrap();

        let candidate = credential(3);
        let proposal_id = ProposalId { transaction_id: Hash::new([4; 32]), proposal_index: 0 };
        let mut proposal: Proposal = run_strategy(any::<Proposal>());
        proposal.gov_action = GovernanceAction::UpdateCommittee(
            None,
            Set::default(),
            vec![(candidate, Epoch::from(99))].try_into().unwrap(),
            run_strategy(any::<UnitRationalNumber>()),
        );
        let mut candidate_fragment = VolatileFragment::default();
        candidate_fragment.proposals.insert(
            proposal_id,
            Arc::new(ProposalState { proposed_in: ProposalPointer::default(), valid_until: Epoch::from(99), proposal }),
        );

        let fragments = [resignation, candidate_fragment];
        let (base, mut appended) = databases_with_base(&base_fragment);
        let view = VolatileMempool::new(&base, fragments.iter());
        append_fragments(&mut appended, &fragments);

        assert_eq!(view.resolve_cc_members(), appended.resolve_cc_members());
    }

    #[test]
    fn treasury_excludes_mempool_donations() {
        let mut fragment = VolatileFragment::default();
        fragment.donations = 42_000;
        let base = VolatileDB::default();
        let view = VolatileMempool::new(&base, [&fragment]);

        assert_eq!(view.resolve_treasury(&Pots::default()), base.resolve_treasury(&Pots::default()));
        assert_eq!(view.resolve_donations(), 42_000);
    }

    fn databases_with_base(fragment: &VolatileFragment) -> (VolatileDB, VolatileDB) {
        let mut base = VolatileDB::default();
        let mut appended = VolatileDB::default();
        let block = anchored(10, fragment.clone());
        base.push_back(block.clone());
        appended.push_back(block);
        (base, appended)
    }

    fn append_fragments(db: &mut VolatileDB, fragments: &[VolatileFragment]) {
        for (offset, fragment) in fragments.iter().cloned().enumerate() {
            db.push_back(anchored(20 + offset as u64, fragment));
        }
    }

    fn anchored(slot: u64, fragment: VolatileFragment) -> AnchoredVolatileFragment {
        let mut block = AnchoredVolatileFragment::fixture(slot, 1);
        block.fragment = fragment;
        block
    }

    fn credential(tag: u8) -> Credential {
        Credential::KeyHash(Hash::new([tag; 28]))
    }
}
