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

use crate::{
    BootstrapWitness, Debug, HasScriptHash, MemoizedNativeScript, MemoizedPlutusData, NonEmptySet,
    PlutusScript, ScriptHash, ScriptKind,
    cbor::{Decode, Encode},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Encode, Decode, Debug, PartialEq, Clone)]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    pub vkeywitness: Option<NonEmptySet<pallas_primitives::conway::VKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptySet<MemoizedNativeScript>>,

    #[n(2)]
    pub bootstrap_witness: Option<NonEmptySet<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<NonEmptySet<MemoizedPlutusData>>,

    #[n(5)]
    pub redeemer: Option<pallas_primitives::conway::Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<3>>>,
}

impl WitnessSet {
    /// Collect provided scripts and compute each ScriptHash in a witness set
    pub fn get_provided_scripts(&self) -> BTreeMap<ScriptHash, ScriptKind> {
        let mut provided_scripts = BTreeMap::new();

        if let Some(native_scripts) = self.native_script.as_ref() {
            provided_scripts.extend(
                native_scripts
                    .iter()
                    .map(|native_script| (native_script.script_hash(), ScriptKind::Native)),
            )
        };

        collect_plutus_scripts(
            &mut provided_scripts,
            self.plutus_v1_script.as_ref(),
            ScriptKind::PlutusV1,
        );

        collect_plutus_scripts(
            &mut provided_scripts,
            self.plutus_v2_script.as_ref(),
            ScriptKind::PlutusV2,
        );

        collect_plutus_scripts(
            &mut provided_scripts,
            self.plutus_v3_script.as_ref(),
            ScriptKind::PlutusV3,
        );

        provided_scripts
    }
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

/// A helper function, generic in the script VERSION, for collecting scripts from witnesses.
fn collect_plutus_scripts<const VERSION: usize>(
    accum: &mut BTreeMap<ScriptHash, ScriptKind>,
    scripts: Option<&NonEmptySet<PlutusScript<VERSION>>>,
    kind: ScriptKind,
) {
    if let Some(plutus_scripts) = scripts {
        accum.extend(
            plutus_scripts
                .iter()
                .map(|script| (script.script_hash(), kind)),
        )
    }
}
