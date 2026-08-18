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

use crate::{Network, ShelleyDelegationPart, ShelleyPaymentPart, StakeAddress, StakePayload, bech32};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct ShelleyAddress(Network, ShelleyPaymentPart, ShelleyDelegationPart);

impl ShelleyAddress {
    pub fn new(network: Network, payment: ShelleyPaymentPart, delegation: ShelleyDelegationPart) -> Self {
        Self(network, payment, delegation)
    }

    /// Indicates if either the payment or delegation part is a script
    pub fn has_script(&self) -> bool {
        self.payment().is_script() || self.delegation().is_script()
    }

    /// Gets the network assoaciated with this address
    pub fn network(&self) -> Network {
        self.0
    }

    /// Gets a numeric id describing the type of the address
    fn typeid(&self) -> u8 {
        match (&self.1, &self.2) {
            (ShelleyPaymentPart::Key(_), ShelleyDelegationPart::Key(_)) => 0b0000,
            (ShelleyPaymentPart::Script(_), ShelleyDelegationPart::Key(_)) => 0b0001,
            (ShelleyPaymentPart::Key(_), ShelleyDelegationPart::Script(_)) => 0b0010,
            (ShelleyPaymentPart::Script(_), ShelleyDelegationPart::Script(_)) => 0b0011,
            (ShelleyPaymentPart::Key(_), ShelleyDelegationPart::Pointer(_)) => 0b0100,
            (ShelleyPaymentPart::Script(_), ShelleyDelegationPart::Pointer(_)) => 0b0101,
            (ShelleyPaymentPart::Key(_), ShelleyDelegationPart::Null) => 0b0110,
            (ShelleyPaymentPart::Script(_), ShelleyDelegationPart::Null) => 0b0111,
        }
    }

    fn as_header(&self) -> u8 {
        let type_id = self.typeid();
        let type_id = type_id << 4;
        let network = u8::from(self.0);
        type_id | network
    }

    pub fn payment(&self) -> &ShelleyPaymentPart {
        &self.1
    }

    pub fn delegation(&self) -> &ShelleyDelegationPart {
        &self.2
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let header = self.as_header();
        let payment = self.1.to_vec();
        let delegation = self.2.to_vec();

        [&[header], payment.as_slice(), delegation.as_slice()].concat()
    }

    pub fn to_hex(&self) -> String {
        let bytes = self.to_vec();
        hex::encode(bytes)
    }

    pub fn to_bech32(&self) -> String {
        let hrp = match &self.0 {
            Network::Testnet => *bech32::HRP_ADDR_TEST,
            Network::Mainnet => *bech32::HRP_ADDR,
        };

        bech32::encode(hrp, self.to_vec())
            .unwrap_or_else(|| unreachable!("shelley address can always be encoded to bech32"))
    }
}

impl TryFrom<ShelleyAddress> for StakeAddress {
    type Error = ();

    fn try_from(addr: ShelleyAddress) -> Result<Self, ()> {
        let payload = match addr.delegation() {
            ShelleyDelegationPart::Key(h) => Ok(StakePayload::Key(*h)),
            ShelleyDelegationPart::Script(h) => Ok(StakePayload::Script(*h)),
            ShelleyDelegationPart::Null | ShelleyDelegationPart::Pointer(..) => Err(()),
        }?;

        Ok(Self::new(addr.network(), payload))
    }
}
