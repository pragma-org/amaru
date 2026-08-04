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

#[cfg(any(test, feature = "test-utils"))]
pub use cbor_array::CborArray;
#[cfg(any(test, feature = "test-utils"))]
pub use cbor_map::CborMap;
pub use serialised_as_array::SerialisedAsArray;
pub use serialised_as_cbor::SerialisedAsCbor;
pub use serialised_as_millis::SerialisedAsMillis;
pub use serialised_as_pico::SerialisedAsPico;
pub use serialised_as_set::SerialisedAsSet;
pub use skip::Skip;

mod serialised_as_array;
mod serialised_as_cbor;
mod serialised_as_millis;
mod serialised_as_pico;
mod serialised_as_set;
mod skip;

#[cfg(any(test, feature = "test-utils"))]
mod cbor_array;
#[cfg(any(test, feature = "test-utils"))]
mod cbor_map;
