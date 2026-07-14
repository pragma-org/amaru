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

use amaru_kernel::{Epoch, StakeCredential};

/// `StableDeregistrations` answer the question "is this account deregistered, as far as the stable db is concerned?"
///
/// In order to avoid accessing the database, this data structure is updated every time we process an
/// block, with member accounts that have been registered or de-registered.
///
/// The epoch at which we learn this information for a given account is stored alongside the account
/// so that we can prune the data that goes outside the reward window. If an account has been
/// deregistered for the length of the reward window, then it cannot get any rewards and
/// doesn't need to be considered in the computation of unclaimed rewards.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StableDeregistrations {
    /// Credential to the epoch in which it was most recently deregistered, for credentials that have
    /// not since been re-registered.
    unregistered_since: BTreeMap<StakeCredential, Epoch>,

    /// Earliest epoch for which this process has fed stable blocks. Until it predates a reward
    /// window, the accumulator is missing older deregistrations (a restart rewinds only `k` blocks,
    /// far less than the window), so boundary checks against it must be skipped.
    tracked_since: Option<Epoch>,
}

impl StableDeregistrations {
    /// Record that `credential` was (re-)registered; it is no longer considered deregistered.
    pub fn register(&mut self, credential: &StakeCredential) {
        self.unregistered_since.remove(credential);
    }

    /// Record that `credential` was deregistered during `epoch`. A subsequent deregistration of the
    /// same credential (after a re-registration) overwrites the recorded epoch, keeping only the most
    /// recent one.
    pub fn deregister(&mut self, credential: StakeCredential, epoch: Epoch) {
        self.unregistered_since.insert(credential, epoch);
    }

    // Stable deregistrations
    pub fn unregistered_accounts(&self) -> impl Iterator<Item = &StakeCredential> {
        self.unregistered_since.keys()
    }

    /// A credential is still registered if it hasn't been deregistered and not re-registered
    pub fn is_registered(&self, credential: &StakeCredential) -> bool {
        !self.unregistered_since.contains_key(credential)
    }

    /// Drop deregistrations that occurred strictly before `oldest`, i.e. that fell out of the reward
    /// window and can no longer contribute to any future unclaimed-rewards determination.
    pub fn prune(&mut self, oldest: Epoch) {
        self.unregistered_since.retain(|_, epoch| *epoch >= oldest);
    }

    /// Record that a stable block from `epoch` has been applied to the ledger
    pub fn track(&mut self, epoch: Epoch) {
        self.tracked_since.get_or_insert(epoch);
    }

    /// Whether this struct has been updated continuously since `epoch`
    pub fn covers(&self, epoch: Epoch) -> bool {
        self.tracked_since.map(|first| first <= epoch).unwrap_or(false)
    }
}

#[cfg(test)]
impl StableDeregistrations {
    pub fn is_empty(&self) -> bool {
        self.unregistered_since.is_empty()
    }

    pub fn len(&self) -> usize {
        self.unregistered_since.len()
    }
}

#[cfg(test)]
mod test {
    use amaru_kernel::Hash;

    use super::*;

    #[test]
    fn deregistered_credentials_are_tracked() {
        let mut deregistrations = StableDeregistrations::default();
        deregistrations.deregister(credentials(1), Epoch::from(10));

        assert!(!deregistrations.is_registered(&credentials(1)), "a deregistered account is no longer registered");
        assert!(deregistrations.is_registered(&credentials(2)), "an untouched account is still registered");
    }

    #[test]
    fn re_registration_clears_a_deregistration() {
        let mut deregistrations = StableDeregistrations::default();
        deregistrations.deregister(credentials(1), Epoch::from(10));
        deregistrations.register(&credentials(1));

        assert!(deregistrations.is_registered(&credentials(1)), "a re-registered account is registered again");
        assert!(deregistrations.is_empty());
    }

    #[test]
    fn re_deregistration_after_re_registration_keeps_latest_epoch() {
        let mut deregistrations = StableDeregistrations::default();
        deregistrations.deregister(credentials(1), Epoch::from(10));
        deregistrations.register(&credentials(1));
        deregistrations.deregister(credentials(1), Epoch::from(12));

        // Pruning up to epoch 11 must NOT drop it: its most recent deregistration was in epoch 12.
        deregistrations.prune(Epoch::from(11));
        assert!(
            !deregistrations.is_registered(&credentials(1)),
            "still deregistered: latest deregistration (epoch 12) is retained"
        );
    }

    #[test]
    fn pruning_drops_entries_older_than_the_window() {
        let mut deregistrations = StableDeregistrations::default();
        deregistrations.deregister(credentials(1), Epoch::from(7));
        deregistrations.deregister(credentials(2), Epoch::from(9));

        deregistrations.prune(Epoch::from(9));

        assert!(
            deregistrations.is_registered(&credentials(1)),
            "epoch 7 is outside the [9, _] window, so it is forgotten"
        );
        assert!(!deregistrations.is_registered(&credentials(2)), "epoch 9 is on the window boundary and retained");
    }

    #[test]
    fn register_of_unknown_credential_is_a_noop() {
        let mut deregistrations = StableDeregistrations::default();
        deregistrations.register(&credentials(1));
        assert!(deregistrations.is_empty());
    }

    #[test]
    fn warmth_tracks_the_first_stable_epoch() {
        let mut deregistrations = StableDeregistrations::default();
        assert!(!deregistrations.covers(Epoch::from(10)), "cold before any stable block");

        deregistrations.track(Epoch::from(8));
        deregistrations.track(Epoch::from(9)); // later blocks don't move it back

        assert!(deregistrations.covers(Epoch::from(8)), "window opening at 8 is covered");
        assert!(!deregistrations.covers(Epoch::from(7)), "window opening at 7 predates our data");
    }

    // HELPERS

    fn credentials(b: u8) -> StakeCredential {
        StakeCredential::ScriptHash(Hash::from([b; 28]))
    }
}
