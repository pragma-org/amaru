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

pub use amaru_uplc::machine::PlutusVersion;

// TODO: Unify with amaru-plutus::plutus_data::{IsKnownPlutusVersion} #[doc(hidden)]
#[doc(hidden)]
pub struct KnownPlutusVersion<const V: usize>;

/// A trait to restrict generic parameter `V` on `ToPlutusData` instances
/// to versions we know about.
pub trait IsKnownPlutusVersion {}

/// Turn a type-level Plutus version back to a value-level version.
pub fn reify_plutus_version<const V: usize>() -> Option<PlutusVersion> {
    match V {
        1 => Some(PlutusVersion::V1),
        2 => Some(PlutusVersion::V2),
        3 => Some(PlutusVersion::V3),
        _ => None,
    }
}

/// Plutus Version 1
pub const PLUTUS_V1: KnownPlutusVersion<1> = KnownPlutusVersion;
impl IsKnownPlutusVersion for KnownPlutusVersion<1> {}

/// Plutus Version 2
pub const PLUTUS_V2: KnownPlutusVersion<2> = KnownPlutusVersion;
impl IsKnownPlutusVersion for KnownPlutusVersion<2> {}

/// Plutus Version 3
pub const PLUTUS_V3: KnownPlutusVersion<3> = KnownPlutusVersion;
impl IsKnownPlutusVersion for KnownPlutusVersion<3> {}
