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

pub mod macros;
pub mod script_context;
pub mod to_plutus_data;

#[doc(hidden)]
/// **Unstable internal APIs** - no guarantees, may change at any time.
/// Only use if you're maintaining code in this workspace.
pub mod unstable;

use to_plutus_data::*;

pub fn to_cbor_tag(constr_index: u64) -> Option<u64> {
    if constr_index <= 6 {
        Some(121 + constr_index)
    } else if constr_index <= 127 {
        Some(1280 - 7 + constr_index)
    } else {
        None
    }
}
