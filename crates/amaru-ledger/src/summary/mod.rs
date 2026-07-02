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

pub mod governance;
pub mod rewards;
pub mod serde;
pub mod stake_distribution;

use ::serde::ser::SerializeStruct;
use amaru_kernel::{Address, CertificatePointer, DRep, Lovelace, PoolId, PoolParams, RationalNumber, drep, relay};
use num::{BigUint, rational::Ratio};

use crate::store::columns::*;

// ---------------------------------------------------------------- AccountState

#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct AccountState {
    pub balance: Lovelace,
    pub pool: Option<PoolId>,
    pub drep: Option<DRep>,
}

impl ::serde::Serialize for AccountState {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AccountState", 3)?;
        s.serialize_field("balance", &self.balance)?;
        s.serialize_field("drep", &self.drep.as_ref().map(drep::AsJson))?;
        s.serialize_field("pool", &self.pool.as_ref().map(hex::encode))?;
        s.end()
    }
}

// ------------------------------------------------------------------- PoolState

#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
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
}

impl ::serde::Serialize for PoolState {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("PoolState", 10)?;

        fn as_bech32_addr<S: ::serde::Serializer>(bytes: &[u8]) -> Result<String, S::Error> {
            Address::from_bytes(bytes).and_then(|addr| addr.to_bech32()).map_err(::serde::ser::Error::custom)
        }

        s.serialize_field("blocks_count", &self.blocks_count)?;
        s.serialize_field("cost", &self.parameters.cost)?;
        s.serialize_field("margin", &[self.parameters.margin.numerator, self.parameters.margin.denominator])?;
        s.serialize_field("owners", &self.parameters.owners.iter().map(hex::encode).collect::<Vec<_>>())?;
        s.serialize_field("pledge", &self.parameters.pledge)?;
        s.serialize_field("relays", &self.parameters.relays.iter().map(relay::AsJson).collect::<Vec<_>>())?;
        s.serialize_field("reward_address", &as_bech32_addr::<S>(&self.parameters.reward_account)?)?;
        s.serialize_field("stake", &self.stake)?;
        s.serialize_field("voting_stake", &self.voting_stake)?;
        s.serialize_field("vrf_key_hash", &hex::encode(self.parameters.vrf))?;

        s.end()
    }
}

// ------------------------------------------------------------------------ Pots

#[derive(Debug)]
pub struct Pots {
    /// Value, in Lovelace, of the treasury at a given epoch.
    pub treasury: Lovelace,
    /// Value, in Lovelace, of the reserves at a given epoch.
    pub reserves: Lovelace,
    /// Values, in Lovelace, generated from fees during an epoch.
    pub fees: Lovelace,
}

impl From<&pots::Row> for Pots {
    fn from(pots: &pots::Row) -> Pots {
        Pots { treasury: pots.treasury, reserves: pots.reserves, fees: pots.fees }
    }
}

impl ::serde::Serialize for Pots {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("Pots", 3)?;
        s.serialize_field("treasury", &self.treasury)?;
        s.serialize_field("reserves", &self.reserves)?;
        s.serialize_field("fees", &self.fees)?;
        s.end()
    }
}

// ------------------------------------------------------------------- SafeRatio

pub type SafeRatio = Ratio<BigUint>;

pub fn safe_ratio(numerator: u64, denominator: u64) -> SafeRatio {
    SafeRatio::new(BigUint::from(numerator), BigUint::from(denominator))
}

pub fn into_safe_ratio(ratio: &RationalNumber) -> SafeRatio {
    SafeRatio::new(BigUint::from(ratio.numerator), BigUint::from(ratio.denominator))
}

fn serialize_safe_ratio(r: &SafeRatio) -> String {
    format!("{}/{}", r.numer(), r.denom())
}
