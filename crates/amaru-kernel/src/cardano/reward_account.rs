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

use std::collections::BTreeMap;

pub use pallas_primitives::conway::RewardAccount;
use thiserror::Error;

use crate::{
    Address, Lovelace, NonEmptyKeyValuePairs as PallasNonEmptyKeyValuePairs, PlutusStakeAddress, StakeCredential,
    StakePayload,
};

// This function shouldn't exist and pallas should provide a RewardAccount = (Network,
// StakeCredential) out of the box instead of row bytes.
pub fn reward_account_to_stake_credential(account: &RewardAccount) -> Option<StakeCredential> {
    if let Some(Address::Stake(stake_addr)) = Address::from_bytes(&account[..]) {
        match stake_addr.payload() {
            StakePayload::Key(key) => Some(StakeCredential::AddrKeyhash(*key)),
            StakePayload::Script(script) => Some(StakeCredential::ScriptHash(*script)),
        }
    } else {
        None
    }
}

/// An 'unsafe' version of `reward_account_to_stake_credential` that panics when the given
/// RewardAccount isn't a `StakeCredential`.
#[expect(clippy::panic)]
pub fn expect_stake_credential(account: &RewardAccount) -> StakeCredential {
    reward_account_to_stake_credential(account)
        .unwrap_or_else(|| panic!("unexpected malformed reward account: {:?}", account))
}

/// The reward withdrawals requested by a transaction.
///
/// A map from the [`PlutusStakeAddress`] being withdrawn from to the amount of [`Lovelace`]
/// taken. The [`PlutusStakeAddress`] key supplies the Plutus-canonical ordering, so this
/// `BTreeMap` iterates, and serializes, in the order a script expects; this is the type
/// that wrapper exists to serve.
#[repr(transparent)]
#[derive(Debug, Default)]
pub struct PlutusWithdrawals(BTreeMap<PlutusStakeAddress, Lovelace>);

impl PlutusWithdrawals {
    /// Iterate over each withdrawal as a `(stake address, amount)` pair, in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = (&PlutusStakeAddress, &Lovelace)> {
        self.0.iter()
    }

    /// Iterate over the stake addresses being withdrawn from, in canonical order.
    pub fn keys(&self) -> impl Iterator<Item = &PlutusStakeAddress> {
        self.0.keys()
    }
}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum WithdrawalError {
    #[error("invalid reward account")]
    InvalidRewardAccount,
    #[error("invalid address type: {0}")]
    InvalidAddressType(Address),
}

impl TryFrom<&PallasNonEmptyKeyValuePairs<RewardAccount, Lovelace>> for PlutusWithdrawals {
    type Error = WithdrawalError;

    fn try_from(value: &PallasNonEmptyKeyValuePairs<RewardAccount, Lovelace>) -> Result<Self, Self::Error> {
        let withdrawals = value
            .iter()
            .map(|(reward_account, coin)| {
                let address = Address::from_bytes(reward_account).ok_or(WithdrawalError::InvalidRewardAccount)?;

                if let Address::Stake(reward_account) = address {
                    Ok((PlutusStakeAddress::from(reward_account), *coin))
                } else {
                    Err(WithdrawalError::InvalidAddressType(address))
                }
            })
            .collect::<Result<BTreeMap<_, _>, WithdrawalError>>()?;

        Ok(Self(withdrawals))
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, prop_compose};

    use crate::{Bytes, Hash, RewardAccount, StakeAddress, StakePayload, any_network};

    prop_compose! {
        pub fn any_reward_account()(
            network in any_network(),
            credential in any::<[u8; 28]>(),
            is_script in any::<bool>(),
        ) -> RewardAccount {
            let payload = if is_script {
                StakePayload::Script
            } else {
                StakePayload::Key
            }(Hash::new(credential));

            Bytes::from(StakeAddress::new(network, payload).to_vec())
        }
    }
}
