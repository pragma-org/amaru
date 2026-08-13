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

use std::collections::VecDeque;

use amaru_kernel::{
    CertificatePointer, ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, Epoch, Lovelace, Point, PoolId,
    Pots, ProposalId, StakeCredential, TransactionInput,
};

mod db;
pub use db::{RewardsAtTip, VolatileDB};

mod overlay;
use overlay::StateOverlay;

mod aggregate;
pub use aggregate::VolatileAggregate;

mod bind;
pub use bind::{Bind, Empty};

mod existence;
pub use existence::Existence;

mod resettable;
pub use resettable::Resettable;

pub(crate) mod fragment;
pub use fragment::{
    AnchoredVolatileFragment, BindError, DiffBind, DiffEpochReg, DiffSet, RegisterError, Registrations, StoreUpdate,
    VolatileFragment,
};

mod series;
pub use series::VolatileSeries;

mod view;
pub use view::VolatileView;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    pub use super::fragment::{any_diff_bind, any_diff_set};
}
#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

/// A stake account's accumulated binding: pool/vote delegations, plus the deposit on registration.
pub type AccountBind<'a> = Bind<&'a (PoolId, CertificatePointer), &'a (DRep, CertificatePointer), &'a Lovelace>;

/// A CC member's accumulated binding: the authorized hot credential on the left, the term an election
/// granted on the right. The empty `value` stops a layer superseding the one below it, since either
/// half can be set without the other.
pub type CommitteeMemberBind<'a> = Bind<&'a ConstitutionalCommitteeMemberStatus, &'a Epoch, &'a Empty>;

/// A DRep's accumulated binding: the metadata anchor, plus the registration record. The registration
/// is the queryable value; the anchor is updated independently of registration, so an anchor-only
/// update is a bind-only (`value: None`) change that composes onto the registration from below.
pub type DRepBind<'a> = Bind<&'a Empty, &'a Empty, &'a DRepRegistration>;

/// An outward-facing store API to query the volatile as a store.
pub trait VolatileState {
    // --------------------------------------------------------------------------------------- UTxOs
    type TransactionOutput<'a>
    where
        Self: 'a;
    fn resolve_input<'a>(&'a self, input: &TransactionInput) -> Self::TransactionOutput<'a>;

    // --------------------------------------------------------------------------------------- Pools
    type Pool;
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool;

    // ------------------------------------------------------------------------------------ Accounts
    type Account<'a>
    where
        Self: 'a;
    fn resolve_account<'a>(&'a self, credential: &StakeCredential) -> Self::Account<'a>;
    fn has_withdrawal(&self, credential: &StakeCredential) -> bool;

    // --------------------------------------------------------------------------------------- DReps
    type DRep<'a>
    where
        Self: 'a;
    fn resolve_drep<'a>(&'a self, credential: &StakeCredential) -> Self::DRep<'a>;

    // ----------------------------------------------------------------------------------- CCMembers
    type CCMembers<'a>
    where
        Self: 'a;
    /// Every cold credential these layers can resolve to a member for.
    ///
    /// This is necessary because a hot key authorized or a seat granted at the epoch boundary has
    /// no stable row yet until the end of the stability window, so iterating the store cannot
    /// enumerate these.
    ///
    /// The same credential may come up multiple times.
    fn resolve_cc_members<'a>(&'a self) -> Self::CCMembers<'a>;

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal;
    fn resolve_proposal(&self, proposal_id: &ProposalId) -> Self::Proposal;

    // ---------------------------------------------------------------------------------------- Pots
    fn resolve_treasury(&self, pots: &Pots) -> Lovelace {
        pots.treasury
    }

    /// The donations collected by blocks that are still volatile, and thus not yet reflected in the
    /// stable pots. They are moved into the treasury at the epoch boundary.
    fn resolve_donations(&self) -> Lovelace;
}

/// A sequence-like API used by the VolatileDB and VolatileSeries.
pub trait VolatileSequence {
    type Item;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn view_back(&self) -> Option<&Self::Item>;
    fn view_front(&self) -> Option<&Self::Item>;
    fn has_point(&self, point: &Point) -> bool;

    fn iter(&self) -> impl DoubleEndedIterator<Item = &Self::Item>;
    fn into_iter(self) -> impl DoubleEndedIterator<Item = Self::Item>;

    fn pop_front(&mut self) -> Option<Self::Item>;
    fn push_back(&mut self, item: Self::Item);
}

#[derive(Debug)]
pub struct RollbackGuard<'a> {
    fork_point: &'a Point,
    recovery: VolatileDBRecovery,
}

impl RollbackGuard<'_> {
    pub fn rollback_length(&self) -> usize {
        self.recovery.rollback_length()
    }

    /// Discarded fragments in **tip-first** order (newest undone block first).
    pub fn discarded_tip_first(&self) -> Box<dyn Iterator<Item = &AnchoredVolatileFragment> + '_> {
        match &self.recovery {
            VolatileDBRecovery::RecoverInEpoch { discarded, .. } => Box::new(discarded.iter().rev()),
            VolatileDBRecovery::RecoverAcrossEpoch { old_current, drained, .. } => {
                Box::new(drained.iter().chain(old_current.iter()).rev())
            }
        }
    }
}

/// The discarded parts of a within-window rollback, enough to restore the pre-rollback state after
/// an arbitrary sequence of roll-forwards (a fork switch replays blocks before it may recover).
///
/// Recovery works by first stripping the replayed blocks (rolling the live series back to
/// `fork_point`) and then re-attaching what was discarded and restoring the overlay snapshot. Only
/// the overlay is cloned; the series parts are moved out at rollback time.
#[derive(Debug)]
pub enum VolatileDBRecovery {
    /// The rollback stayed within the current epoch: only a suffix of `current` was removed.
    RecoverInEpoch { discarded: VecDeque<AnchoredVolatileFragment>, overlay: StateOverlay },
    /// The rollback crossed the epoch boundary: the opening-epoch `current` was discarded and the
    /// `draining` series was promoted and split.
    RecoverAcrossEpoch {
        old_current: VolatileSeries,
        drained: VecDeque<AnchoredVolatileFragment>,
        overlay: StateOverlay,
    },
}

impl VolatileDBRecovery {
    fn rollback_length(&self) -> usize {
        match self {
            Self::RecoverInEpoch { discarded, .. } => discarded.len(),
            Self::RecoverAcrossEpoch { old_current, drained, .. } => old_current.len() + drained.len(),
        }
    }
}
