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

use std::cmp::Ordering;

use crate::{Network, StakePayload, bech32};

/// A decoded Stake address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct StakeAddress(Network, StakePayload);

impl StakeAddress {
    pub fn new(network: Network, payload: StakePayload) -> Self {
        Self(network, payload)
    }

    /// Gets the network assoaciated with this address
    pub fn network(&self) -> Network {
        self.0
    }

    /// Gets a numeric id describing the type of the address
    pub fn typeid(&self) -> u8 {
        match &self.1 {
            StakePayload::Key(_) => 0b1110,
            StakePayload::Script(_) => 0b1111,
        }
    }

    /// Builds the header for this address
    pub fn to_header(&self) -> u8 {
        let type_id = self.typeid();
        let type_id = type_id << 4;
        let network = u8::from(self.0);

        type_id | network
    }

    /// Gets the payload of this address
    pub fn payload(&self) -> &StakePayload {
        &self.1
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let header = self.to_header();
        [&[header], self.1.as_ref()].concat()
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_vec())
    }

    pub fn to_bech32(&self) -> String {
        let hrp = match &self.0 {
            Network::Testnet => *bech32::HRP_STAKE_TEST,
            Network::Mainnet => *bech32::HRP_STAKE,
        };

        bech32::encode(hrp, self.to_vec())
            .unwrap_or_else(|| unreachable!("stake address can always be encoded to bech32"))
    }

    pub fn is_script(&self) -> bool {
        self.payload().is_script()
    }
}

/// A stake address with ordering the way Plutus expects withdrawal keys to be sorted.
///
/// A wrapper around [`StakeAddress`] to provide a custom [`Ord`] implementation.
/// Wrapping the address makes a `BTreeMap<PlutusStakeAddress, _>` iterate, and therefore serialize,
/// in the order a script expects. Equality is defined to agree with this ordering.
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct PlutusStakeAddress(StakeAddress);

impl From<StakeAddress> for PlutusStakeAddress {
    fn from(value: StakeAddress) -> Self {
        Self(value)
    }
}

impl From<PlutusStakeAddress> for StakeAddress {
    fn from(value: PlutusStakeAddress) -> Self {
        value.0
    }
}

impl AsRef<StakeAddress> for PlutusStakeAddress {
    fn as_ref(&self) -> &StakeAddress {
        &self.0
    }
}

impl Ord for PlutusStakeAddress {
    /// Plutus canonically expects stake addresses to be sorted by network,
    /// then script credentials > public key credentials,
    /// and finally lexicographical ordering of hash bytes.
    ///
    ///
    /// [Aiken reference implementation](https://github.com/aiken-lang/aiken/blob/a8c032935dbaf4a1140e9d8be5c270acd32c9e8c/crates/uplc/src/tx/script_context.rs#L1112)
    fn cmp(&self, other: &Self) -> Ordering {
        if self.0.network() != other.0.network() {
            return self.0.network().cmp(&other.0.network());
        }

        // TODO: Move to StakePayload?
        match (self.0.payload(), other.0.payload()) {
            (StakePayload::Script(..), StakePayload::Key(..)) => Ordering::Less,
            (StakePayload::Key(..), StakePayload::Script(..)) => Ordering::Greater,
            (StakePayload::Script(hash_a), StakePayload::Script(hash_b)) => hash_a.cmp(hash_b),
            (StakePayload::Key(hash_a), StakePayload::Key(hash_b)) => hash_a.cmp(hash_b),
        }
    }
}

impl PartialOrd for PlutusStakeAddress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for PlutusStakeAddress {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PlutusStakeAddress {}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::any_network;

    fn stake_address_strategy() -> impl Strategy<Value = PlutusStakeAddress> {
        (prop::bool::ANY, any::<[u8; 28]>(), any_network()).prop_map(|(is_script, hash_bytes, network)| {
            let delegation: StakePayload =
                if is_script { StakePayload::Script(hash_bytes.into()) } else { StakePayload::Key(hash_bytes.into()) };

            PlutusStakeAddress(StakeAddress::new(network, delegation))
        })
    }

    #[test]
    fn proptest_stake_address_ordering() {
        proptest!(|(addresses in prop::collection::vec(stake_address_strategy(), 20..100))| {
            let mut sorted = addresses.clone();
            sorted.sort();


            for window in sorted.windows(2) {
                let a = &window[0];
                let b = &window[1];

                let net_a = a.0.network();
                let net_b = b.0.network();


                // We sort by network first (testnet, mainnet, other by tag)
                if net_a != net_b {
                    prop_assert!(
                        net_a < net_b,
                        "Network ordering violated: {:?} should be < {:?}",
                        u8::from(net_a),
                        u8::from(net_b)
                    );
                } else {
                    match (a.0.payload(), b.0.payload()) {
                        // Script < Stake
                        (StakePayload::Script(_), StakePayload::Key(_)) => {
                            // This is correct
                        }
                        (StakePayload::Key(_), StakePayload::Script(_)) => {
                            prop_assert!(false, "Payload type ordering violated: Key should not come before Script");
                        }
                        // Same payload compare bytes
                        (StakePayload::Script(h1), StakePayload::Script(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Script hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                        (StakePayload::Key(h1), StakePayload::Key(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Stake hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                    }
                }
            }
        });
    }
}
