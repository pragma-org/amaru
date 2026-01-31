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

use crate::{AsShelley, HasOwnership, StakeCredential};

pub use pallas_addresses::Address;

pub fn is_locked_by_script(address: &Address) -> bool {
    matches!(
        address.as_shelley().map(|addr| addr.owner()),
        Some(StakeCredential::ScriptHash(_))
    )
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use crate::{
        Address, Network, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart, any_hash28,
    };
    use proptest::prelude::*;

    pub fn any_shelley_address() -> impl Strategy<Value = Address> {
        (any::<bool>(), any_hash28(), any_hash28()).prop_map(
            |(is_mainnet, payment_hash, delegation_hash)| {
                let network = if is_mainnet {
                    Network::Mainnet
                } else {
                    Network::Testnet
                };

                let payment = ShelleyPaymentPart::Key(payment_hash);
                let delegation = ShelleyDelegationPart::Key(delegation_hash);

                Address::Shelley(ShelleyAddress::new(network, payment, delegation))
            },
        )
    }
}
