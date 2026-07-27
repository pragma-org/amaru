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
    Anchor, CertificatePointer, ComparableProposalId, DRep, DRepRegistration, Epoch, Lovelace, Point, PoolId,
    StakeCredential, TransactionInput,
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

/// A CC member's accumulated binding: the hot-key delegation. Membership and term come from below,
/// since no in-block cert establishes them.
pub type CommitteeMemberBind<'a> = Bind<&'a StakeCredential, &'a Empty, &'a Epoch>;

/// A DRep's accumulated binding: the metadata anchor, plus the registration record. The registration
/// is the queryable value; the anchor is updated independently of registration, so an anchor-only
/// update is a bind-only (`value: None`) change that composes onto the registration from below.
pub type DRepBind<'a> = Bind<&'a Anchor, &'a Empty, &'a DRepRegistration>;

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
    type CCMember<'a>
    where
        Self: 'a;
    fn resolve_cc_member<'a>(&'a self, credential: &StakeCredential) -> Self::CCMember<'a>;

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal;
    fn resolve_proposal(&self, proposal_id: &ComparableProposalId) -> Self::Proposal;
}

/// A sequence-like API used by the VolatileDB and VolatileSeries.
pub trait VolatileSequence {
    type Item;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn view_back(&self) -> Option<&Self::Item>;
    fn view_front(&self) -> Option<&Self::Item>;
    fn has_point(&self, point: &Point) -> bool;

    fn iter(&self) -> impl Iterator<Item = &Self::Item>;
    fn into_iter(self) -> impl Iterator<Item = Self::Item>;

    fn pop_front(&mut self) -> Option<Self::Item>;
    fn push_back(&mut self, item: Self::Item);
}

#[derive(Debug)]
pub struct RollbackGuard<'a> {
    fork_point: &'a Point,
    recovery: VolatileDBRecovery,
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
