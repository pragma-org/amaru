// Copyright 2024 PRAGMA
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

// TODO: Temporary re-exports until Pallas migrations
//
// Re-exports still needed in a few places; but that shall become redundant as soon as we have
// properly reworked addresses.
pub use pallas_addresses::{
    ByronAddress, Error as AddressError, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart,
    StakeAddress, StakePayload,
    byron::{AddrAttrProperty, AddrType, AddressPayload},
};

// TODO: Temporary re-exports until Pallas migrations
//
// See above.
pub use pallas_primitives::conway::{Constr, KeepRaw, MaybeIndefArray};

// TODO: Temporary re-exports until Pallas migrations
//
// See above.
pub use pallas_traverse::{ComputeHash, OriginalHash};

// TODO: Interalize amaru-slot-arithmetic within amaru-kernel
pub use amaru_slot_arithmetic::*;

pub mod cardano;
pub use cardano::*;

pub mod cbor {
    pub use amaru_minicbor_extra::*;
    pub use minicbor::{
        data::{IanaTag, Tag, Type},
        *,
    };
    pub use pallas_codec::utils::AnyCbor as Any;
}
pub use cbor::{from_cbor, from_cbor_no_leftovers, to_cbor};

pub mod data_structures;
pub use data_structures::*;

pub use serde_json as json;

pub mod macros;

pub mod traits;
pub use traits::*;

pub mod utils;
