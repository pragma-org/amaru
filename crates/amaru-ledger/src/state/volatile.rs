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

use std::{collections::VecDeque, fmt};

use amaru_kernel::{
    CertificatePointer, ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, Epoch, Lovelace, Point, PoolId,
    Pots, ProposalId, StakeCredential, TransactionInput,
};

use crate::store::columns::pools_vrf;

mod db;
pub use db::{RewardsAtTip, VolatileDB, VrfOccupancy};

mod overlay;
use overlay::StateOverlay;

mod aggregate;
pub use aggregate::{IndexedBind, VolatileAggregate};

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

/// A set-shaped fragment diff holding one slot per key, so restating a key supersedes its earlier
/// verdict in place.
///
/// The payload sits in the *left* slot, not in `value`: [`DiffBind::register`] rejects a key already
/// registered in the same diff, whereas [`DiffBind::bind_left`] overwrites.
pub type DiffLeftBind<K, V> = DiffBind<K, V, Empty, Empty>;

/// The aggregate counterpart of [`DiffLeftBind`].
pub type IndexedSet<K, V> = IndexedBind<K, V, Empty, Empty>;

/// Claim and release keys in a [`DiffLeftBind`], each write superseding any earlier verdict on that
/// key.
pub trait DiffLeftBindExt<K, V> {
    fn produce(&mut self, key: K, value: V);

    fn consume(&mut self, key: K);
}

impl<K: Ord + fmt::Debug, V> DiffLeftBindExt<K, V> for DiffLeftBind<K, V> {
    fn produce(&mut self, key: K, value: V) {
        self.bind_left(key, Some(value)).unwrap_or_else(|err| unreachable!("a DiffLeftBind never unregisters: {err:?}"))
    }

    fn consume(&mut self, key: K) {
        self.bind_left(key, None).unwrap_or_else(|err| unreachable!("a DiffLeftBind never unregisters: {err:?}"))
    }
}

/// Read a [`DiffLeftBind`] or [`IndexedSet`] verdict out of the left slot. `Unchanged` maps to
/// `Unknown`, though no [`DiffLeftBindExt`] write produces it.
pub fn left_verdict<V: Copy>(verdict: Existence<Bind<&V, &Empty, &Empty>>) -> Existence<V> {
    match verdict {
        Existence::Unknown => Existence::Unknown,
        Existence::Gone => Existence::Gone,
        Existence::Exists(bind) => match bind.left {
            Resettable::Set(value) => Existence::Exists(*value),
            Resettable::Reset => Existence::Gone,
            Resettable::Unchanged => Existence::Unknown,
        },
    }
}

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

/// The volatile layers' verdict on a pool's VRF keys `current` projects the active parameters'
/// key, the only one exempt when the pool itself re-registers, and `pending` projects
/// a not-yet-activated re-registration's key. For either, `Unknown`
/// defers to the stable row, while `Gone` settles the answer as "none": a boundary event
/// invalidated whatever the stale stable row still shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolatilePoolVrfs {
    pub current: Existence<pools_vrf::Key>,
    pub pending: Existence<pools_vrf::Key>,
}

/// An outward-facing store API to query the volatile as a store.
pub trait VolatileState {
    // --------------------------------------------------------------------------------------- UTxOs
    type TransactionOutput<'a>
    where
        Self: 'a;
    #[expect(clippy::panic)]
    fn resolve_input<'a>(&'a self, input: &TransactionInput) -> Self::TransactionOutput<'a> {
        panic!("VolatileState.resolve_input({input})")
    }

    // --------------------------------------------------------------------------------------- Pools
    type Pool;
    #[expect(clippy::panic)]
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool {
        panic!("VolatileState.resolve_pool({pool_id})")
    }

    type PoolVrfs;
    #[expect(clippy::panic)]
    fn resolve_pool_vrfs(&self, pool_id: PoolId) -> Self::PoolVrfs {
        panic!("VolatileState.resolve_pool_vrfs({pool_id})")
    }

    // ------------------------------------------------------------------------------ VRF key hashes
    type VrfKeyHash;
    #[expect(clippy::panic)]
    fn resolve_vrf_key_hash(&self, vrf: &pools_vrf::Key) -> Self::VrfKeyHash {
        panic!("VolatileState.resolve_vrf_key_hash({vrf})")
    }

    // ------------------------------------------------------------------------------------ Accounts
    type Account<'a>
    where
        Self: 'a;
    #[expect(clippy::panic)]
    fn resolve_account<'a>(&'a self, credential: &StakeCredential) -> Self::Account<'a> {
        panic!("VolatileState.resolve_account({credential})")
    }
    #[expect(clippy::panic)]
    fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        panic!("VolatileState.has_withdrawal({credential})")
    }

    // --------------------------------------------------------------------------------------- DReps
    type DRep<'a>
    where
        Self: 'a;
    #[expect(clippy::panic)]
    fn resolve_drep<'a>(&'a self, credential: &StakeCredential) -> Self::DRep<'a> {
        panic!("VolatileState.resolve_drep({credential})")
    }

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
    #[expect(clippy::panic)]
    fn resolve_cc_members<'a>(&'a self) -> Self::CCMembers<'a> {
        panic!("VolatileState.resolve_cc_members()")
    }

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal;
    #[expect(clippy::panic)]
    fn resolve_proposal(&self, proposal_id: &ProposalId) -> Self::Proposal {
        panic!("VolatileState.resolve_proposal({proposal_id})")
    }

    // ---------------------------------------------------------------------------------------- Pots
    #[expect(clippy::panic)]
    fn resolve_treasury(&self, pots: &Pots) -> Lovelace {
        panic!("VolatileState.resolve_treasury({pots:?})")
    }

    /// The donations collected by blocks that are still volatile, and thus not yet reflected in the
    /// stable pots. They are moved into the treasury at the epoch boundary.
    #[expect(clippy::panic)]
    fn resolve_donations(&self) -> Lovelace {
        panic!("VolatileState.resolve_donations()")
    }
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
