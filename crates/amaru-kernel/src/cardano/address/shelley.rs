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

use crate::{AsHash, Credential, Network, RewardAccount, StakeReference, bech32};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct ShelleyAddress {
    network: Network,
    payment: Credential,
    delegation: Option<StakeReference>,
}

impl ShelleyAddress {
    pub fn new(network: Network, payment: Credential, delegation: Option<StakeReference>) -> Self {
        Self { network, payment, delegation }
    }

    /// Indicates if either the payment or delegation part is a script
    pub fn has_script(&self) -> bool {
        self.payment().is_script() || self.delegation().is_some_and(|delegation| delegation.is_script())
    }

    /// Gets the network assoaciated with this address
    pub fn network(&self) -> Network {
        self.network
    }

    /// Gets a numeric id describing the type of the address
    fn typeid(&self) -> u8 {
        let payment_bit = if self.payment.is_script() { 0b0001 } else { 0b0000 };
        let delegation_bits = match &self.delegation {
            Some(StakeReference::Credential(credential)) if credential.is_script() => 0b0010,
            Some(StakeReference::Credential(_)) => 0b0000,
            Some(StakeReference::Pointer(_)) => 0b0100,
            None => 0b0110,
        };

        payment_bit | delegation_bits
    }

    fn as_header(&self) -> u8 {
        let type_id = self.typeid();
        let type_id = type_id << 4;
        let network = u8::from(self.network);
        type_id | network
    }

    pub fn payment(&self) -> &Credential {
        &self.payment
    }

    pub fn delegation(&self) -> Option<&StakeReference> {
        self.delegation.as_ref()
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let header = self.as_header();
        let payment = self.payment.as_hash();
        let delegation = self.delegation.map(|delegation| delegation.to_vec()).unwrap_or_default();

        [&[header], payment.as_ref(), delegation.as_slice()].concat()
    }

    pub fn to_hex(&self) -> String {
        let bytes = self.to_vec();
        hex::encode(bytes)
    }

    pub fn to_bech32(&self) -> String {
        let hrp = match &self.network {
            Network::Testnet => *bech32::HRP_ADDR_TEST,
            Network::Mainnet => *bech32::HRP_ADDR,
        };

        bech32::encode(hrp, self.to_vec())
            .unwrap_or_else(|| unreachable!("shelley address can always be encoded to bech32"))
    }
}

impl TryFrom<ShelleyAddress> for RewardAccount {
    type Error = ();

    fn try_from(addr: ShelleyAddress) -> Result<Self, ()> {
        let credential = addr.delegation().and_then(StakeReference::credential).ok_or(())?;

        Ok(Self::new(addr.network(), credential))
    }
}
