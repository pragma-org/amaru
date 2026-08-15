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

use std::ops::Deref;

use ::serde::ser::SerializeStruct;
use amaru_kernel::{CertificatePointer, DRep, Lovelace, PoolId, PoolParams, SafeRatio, drep, relay};
use num::rational::Ratio;

pub mod governance;
pub mod rewards;
pub mod serde;
pub mod stake_distribution;

// ---------------------------------------------------------------- AccountState

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AccountState {
    pub balance: Lovelace,
    pub rewards: Lovelace,
    pub pool: Option<PoolId>,
    pub drep: Option<DRep>,
}

impl AccountState {
    pub fn with_rewards(self, rewards: Lovelace) -> Self {
        Self { rewards, ..self }
    }
}

impl ::serde::Serialize for AccountState {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AccountState", 3)?;
        s.serialize_field("balance", &self.balance)?;
        // rewards are ignored in this serialization
        s.serialize_field("drep", &self.drep.as_ref().map(drep::AsJson))?;
        s.serialize_field("pool", &self.pool.as_ref().map(hex::encode))?;
        s.end()
    }
}

// ------------------------------------------------------------------- PoolState

#[derive(Debug, Clone)]
pub struct PoolState {
    /// Date since the pool last registered. Pool updates do not influence this pointer; but pool
    /// de-registration would cause it to reset on the next registration.
    pub registered_at: CertificatePointer,

    /// Number of blocks produced during an epoch by the underlying pool.
    pub blocks_count: u64,

    /// The stake used for verifying the leader-schedule.
    pub stake: Lovelace,

    /// The stake used when counting votes, which includes proposal deposits for proposals whose
    /// refund address is delegated to the underlying pool.
    pub voting_stake: Lovelace,

    /// The pool's margin, as define per its last registration certificate.
    ///
    /// TODO: The margin is already present within the parameters, but is pre-computed here as a
    /// SafeRatio to avoid unnecessary recomputations during rewards calculations. Arguably, it
    /// should just be stored as a `SafeRatio` from within `PoolParams` to begin with!
    pub margin: SafeRatio,

    /// The pool's parameters, as define per its last registration certificate.
    pub parameters: PoolParams,

    /// Delegate representative to fall back to when the operator does not cast an explicit vote.
    ///
    /// This is derived from the pool reward account delegation in the corresponding stake
    /// distribution snapshot. It is not serialized because it is only needed at runtime for
    /// ratification.
    pub fallback_drep: Option<DRep>,
}

impl ::serde::Serialize for PoolState {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolState", 10)?;

        // Force reduction of the numerator and denominator
        let r = Ratio::new(self.parameters.margin.numerator, self.parameters.margin.denominator);

        s.serialize_field("blocks_count", &self.blocks_count)?;
        s.serialize_field("cost", &self.parameters.cost)?;
        s.serialize_field("margin", &[r.numer(), r.denom()])?;
        s.serialize_field("metadata", &self.parameters.metadata)?;
        s.serialize_field("owners", &self.parameters.owners.iter().map(hex::encode).collect::<Vec<_>>())?;
        s.serialize_field("pledge", &self.parameters.pledge)?;
        s.serialize_field("relays", &self.parameters.relays.iter().map(relay::AsJson).collect::<Vec<_>>())?;
        s.serialize_field("reward_address", &hex::encode(self.parameters.reward_account.deref()))?;
        s.serialize_field("stake", &self.stake)?;
        s.serialize_field("voting_stake", &self.voting_stake)?;
        s.serialize_field("vrf_key_hash", &hex::encode(self.parameters.vrf))?;

        s.end()
    }
}
