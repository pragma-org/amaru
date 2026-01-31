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

use crate::{AddrAttrProperty, Address, AddressPayload, ByronAddress, Network, from_cbor};

pub trait HasNetwork {
    /// Returns the Network of a given entity
    fn has_network(&self) -> Network;
}

impl HasNetwork for Address {
    #[expect(clippy::unwrap_used)]
    fn has_network(&self) -> Network {
        match self {
            Address::Byron(address) => address.has_network(),
            // Safe to unwrap here, as there will always be a network value for self.network as long as it is not Address::Byron (which is handled above)
            Address::Shelley(_) | Address::Stake(_) => self.network().unwrap(),
        }
    }
}

impl HasNetwork for ByronAddress {
    /*
        According to the Byron address specification (https://raw.githubusercontent.com/cardano-foundation/CIPs/master/CIP-0019/CIP-0019-byron-addresses.cddl),
        the attributes can optionally contain a u32 network discriminant, identifying a specific testnet network.

        When decoding Byron address attributes (https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/eras/byron/ledger/impl/src/Cardano/Chain/Common/AddrAttributes.hs#L122-L144),
        the Haskell node defaults NetworkMagic to NetworkMainOrStage, unless otherwise specified. The discriminant can be any `NetworkMagic` (sometimes referred to as `ProtocolMagic`), identifying a specific testnet.
        If present, it is Testnet(discriminant).

        It does not, notabtly, validate this discriminant, as evidenced by this conflicting Byron address on Preprod: 2cWKMJemoBaiqkR9D1YZ2xQ2BhVxzauukrsxm8ttZUrto1f7kr5J1tD9uhtEtTc9U4PuF (found in tx 9738801cc4f7e46bb3561a138a403fa8470e8a4faf2df5009023e7bbcdf09cb4).
        This address encodes a `NetworkMagic` of 1097911063. The `NetworkMagic` of Preprod is 1 (https://book.world.dev.cardano.org/environments/preprod/byron-genesis.json).


        As a result, since we are only checking the network of a Byron address for validation, we will mirror the Haskell node logic and disregard the discriminant when fetching the network from an address.
        (https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/libs/cardano-ledger-core/src/Cardano/Ledger/Address.hs#L152)
    */
    #[expect(clippy::unwrap_used)]
    fn has_network(&self) -> Network {
        // Unwrap is safe, we know that there is a valid address payload if it is a Byron address.
        let x: AddressPayload = from_cbor(&self.payload.0).unwrap();
        for attribute in x.attributes.iter() {
            if let AddrAttrProperty::NetworkTag(_) = attribute {
                // We are ignoring the network discriminant here, as the Haskell node does
                return Network::Testnet;
            }
        }

        Network::Mainnet
    }
}
