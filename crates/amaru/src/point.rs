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

use amaru_kernel::Point;

/// Convert an Amaru's point into a pallas_network's Point.
///
/// TODO: Remove this function by moving the 'Point' type definition downstream from pallas_network
/// to pallas_primitives.
pub fn to_network_point(point: Point) -> pallas_network::miniprotocols::Point {
    match point {
        Point::Origin => pallas_network::miniprotocols::Point::Origin,
        Point::Specific(slot, hash) => pallas_network::miniprotocols::Point::Specific(slot, hash),
    }
}

/// Convert a pallas_network's Point into an Amaru's Point.
///
/// TODO: Remove this function by moving the 'Point' type definition downstream from pallas_network
/// to pallas_primitives.
pub fn from_network_point(point: &pallas_network::miniprotocols::Point) -> Point {
    match point.clone() {
        pallas_network::miniprotocols::Point::Origin => Point::Origin,
        pallas_network::miniprotocols::Point::Specific(slot, hash) => Point::Specific(slot, hash),
    }
}
