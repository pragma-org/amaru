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

use std::{collections::BTreeMap, fmt};

use thiserror::Error;

use crate::{Address, AsHash, Credential, Lovelace, Network, NonEmptyKeyValuePairs, bech32, cbor};

/// The account rewards are paid to, often called a StakeAddress.
///
/// On the wire, a reward account is 29 bytes: a header carrying a stake-address type and
/// the network tag, followed by the 28 byte credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String")]
#[serde(try_from = "String")]
pub struct RewardAccount {
    network: Network,
    credential: Credential,
}

impl RewardAccount {
    pub fn new(network: Network, credential: Credential) -> Self {
        Self { network, credential }
    }

    /// The network tag carried in the account's header.
    pub fn network(&self) -> Network {
        self.network
    }

    /// The stake credential owning the account.
    pub fn credential(&self) -> Credential {
        self.credential
    }

    pub fn is_script(&self) -> bool {
        self.credential.is_script()
    }

    /// Gets a numeric id describing the type of the address
    fn typeid(&self) -> u8 {
        match &self.credential {
            Credential::KeyHash(_) => 0b1110,
            Credential::ScriptHash(_) => 0b1111,
        }
    }

    /// Builds the header for this address
    fn as_header(&self) -> u8 {
        let type_id = self.typeid();
        let type_id = type_id << 4;
        let network = u8::from(self.network);

        type_id | network
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let header = self.as_header();
        [&[header], self.credential.as_hash().as_ref()].concat()
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_vec())
    }

    pub fn to_bech32(&self) -> String {
        let hrp = match &self.network {
            Network::Testnet => *bech32::HRP_STAKE_TEST,
            Network::Mainnet => *bech32::HRP_STAKE,
        };

        bech32::encode(hrp, self.to_vec())
            .unwrap_or_else(|| unreachable!("stake address can always be encoded to bech32"))
    }
}

#[derive(Debug, Error)]
#[error("malformed reward account: {}", hex::encode(.0))]
pub struct MalformedRewardAccount(Vec<u8>);

impl TryFrom<&[u8]> for RewardAccount {
    type Error = MalformedRewardAccount;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        match Address::from_bytes(bytes) {
            Some(Address::Stake(account)) => Ok(account),
            _ => Err(MalformedRewardAccount(bytes.to_vec())),
        }
    }
}

impl From<RewardAccount> for String {
    fn from(account: RewardAccount) -> Self {
        account.to_hex()
    }
}

impl TryFrom<String> for RewardAccount {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let bytes = hex::decode(&value).map_err(|e| e.to_string())?;
        Self::try_from(bytes.as_slice()).map_err(|e| e.to_string())
    }
}

impl fmt::Display for RewardAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl<C> cbor::Encode<C> for RewardAccount {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.bytes(&self.to_vec())?.ok()
    }
}

impl<'d, C: cbor::HasProtocolVersion> cbor::Decode<'d, C> for RewardAccount {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let position = d.position();
        let bytes = cbor::decode_bytes_with(d, ctx)?;
        Self::try_from(bytes.as_ref()).map_err(|e| cbor::decode::Error::message(e.to_string()).at(position))
    }
}

/// The reward withdrawals requested by a transaction.
///
/// A map from the [`RewardAccount`] being withdrawn from to the amount of [`Lovelace`] taken.
#[repr(transparent)]
#[derive(Debug, Default)]
pub struct PlutusWithdrawals(BTreeMap<RewardAccount, Lovelace>);

impl PlutusWithdrawals {
    /// Iterate over each withdrawal as an `(account, amount)` pair, in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = (&RewardAccount, &Lovelace)> {
        self.0.iter()
    }

    /// Iterate over the reward accounts being withdrawn from, in canonical order.
    pub fn keys(&self) -> impl Iterator<Item = &RewardAccount> {
        self.0.keys()
    }
}

impl From<&NonEmptyKeyValuePairs<RewardAccount, Lovelace>> for PlutusWithdrawals {
    fn from(value: &NonEmptyKeyValuePairs<RewardAccount, Lovelace>) -> Self {
        Self(value.iter().map(|(account, coin)| (*account, *coin)).collect())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prop_compose;

    use crate::{RewardAccount, any_credential, any_network};

    prop_compose! {
        pub fn any_reward_account()(
            network in any_network(),
            credential in any_credential(),
        ) -> RewardAccount {
            RewardAccount::new(network, credential)
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use proptest::prelude::*;
    use test_case::test_case;

    use super::{RewardAccount, any_reward_account};
    use crate::{Credential, cbor, prop_cbor_roundtrip, protocol_version};

    prop_cbor_roundtrip!(RewardAccount, any_reward_account());

    fn decode(bytes: &[u8]) -> Result<RewardAccount, cbor::decode::Error> {
        let mut ctx = protocol_version::MINIMUM_SUPPORTED;
        let mut d = cbor::Decoder::new(bytes);
        d.decode_with(&mut ctx)
    }

    #[test_case(&[0xE0; 28]; "missing header byte")]
    #[test_case(&[0xE0; 30]; "trailing byte")]
    #[test_case(&[0xE2; 29]; "network tag greater than one")]
    #[test_case(&[0x61; 29]; "enterprise address header")]
    #[test_case(&[]; "empty")]
    fn rejects_malformed_bytes(payload: &[u8]) {
        let mut bytes = vec![0x58, payload.len() as u8];
        bytes.extend_from_slice(payload);
        let err = decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("malformed reward account"), "{err}");
    }

    #[test_case(&[0xE0; 29]; "testnet key account")]
    #[test_case(&[0xE1; 29]; "mainnet key account")]
    #[test_case(&[0xF0; 29]; "testnet script account")]
    #[test_case(&[0xF1; 29]; "mainnet script account")]
    fn accepts_wellformed_bytes(payload: &[u8]) {
        let mut bytes = vec![0x58, payload.len() as u8];
        bytes.extend_from_slice(payload);
        assert_eq!(decode(&bytes).unwrap().to_vec(), payload);
    }

    /// The order a Plutus script expects withdrawal keys in: network first, then script
    /// credentials before key credentials, then hash bytes.
    ///
    /// [Aiken reference implementation](https://github.com/aiken-lang/aiken/blob/a8c032935dbaf4a1140e9d8be5c270acd32c9e8c/crates/uplc/src/tx/script_context.rs#L1112)
    #[test]
    fn proptest_reward_account_ordering() {
        proptest!(|(accounts in prop::collection::vec(any_reward_account(), 20..100))| {
            let mut sorted = accounts.clone();
            sorted.sort();

            for window in sorted.windows(2) {
                let a = &window[0];
                let b = &window[1];

                if a.network() != b.network() {
                    prop_assert!(
                        a.network() < b.network(),
                        "Network ordering violated: {:?} should be < {:?}",
                        u8::from(a.network()),
                        u8::from(b.network())
                    );
                } else {
                    match (a.credential(), b.credential()) {
                        (Credential::ScriptHash(_), Credential::KeyHash(_)) => {}
                        (Credential::KeyHash(_), Credential::ScriptHash(_)) => {
                            prop_assert!(false, "Key credential should not come before Script credential");
                        }
                        (Credential::ScriptHash(h1), Credential::ScriptHash(h2))
                        | (Credential::KeyHash(h1), Credential::KeyHash(h2)) => {
                            prop_assert!(h1 <= h2, "Hash ordering violated: {:?} should be <= {:?}", h1, h2);
                        }
                    }
                }
            }
        });
    }
}
