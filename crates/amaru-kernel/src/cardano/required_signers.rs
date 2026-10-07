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

use crate::{Hash, data_structures::non_empty_unique_vec::encode_sorted_non_empty_set, size::KEY};

encode_sorted_non_empty_set! {
    /// The public-key hashes a transaction declares it must be signed by.
    ///
    /// The ledger holds this field in a `Set`, so its elements reach the wire in ascending order
    /// however they arrived.
    RequiredSigners(Hash<{ KEY }>)
}
