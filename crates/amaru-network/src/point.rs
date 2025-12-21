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

use amaru_kernel::{Point as KernelPoint, Slot};
use pallas_network::miniprotocols::Point as NetworkPoint;

pub fn to_network_point(point: KernelPoint) -> NetworkPoint {
    match point {
        KernelPoint::Origin => NetworkPoint::Origin,
        KernelPoint::Specific(slot, hash) => NetworkPoint::Specific(slot.as_u64(), hash.to_vec()),
    }
}

pub fn from_network_point(point: &NetworkPoint) -> KernelPoint {
    match point {
        NetworkPoint::Origin => KernelPoint::Origin,
        NetworkPoint::Specific(slot, hash) => {
            KernelPoint::Specific(Slot::from(*slot), From::from(hash.as_slice()))
        }
    }
}
