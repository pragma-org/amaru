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
    BootstrapWitness, HasScriptHash, Hash, MemoizedNativeScript, MemoizedPlutusData, NonEmptySet,
    NonEmptyVec, PlutusScript, Redeemers, ScriptKind, VKeyWitness, cbor, size::SCRIPT,
};
use std::collections::BTreeMap;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    cbor::Encode,
    cbor::Decode,
)]
#[cbor(map)]
pub struct WitnessSet {
    /// FIXME: Accidentally not a set
    ///
    ///   This is supposed to be a NonEmptySet where duplicates would fail to decode. But it isn't.
    ///   In the Haskell's codebsae, the default decoder for Set fails on duplicate starting from
    ///   v9 and above:
    ///
    ///   <https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Decoding/Decoder.hs#L906-L928>.
    ///
    ///   However, the decoder for both key and bootstrap witnesses was (accidentally) manually
    ///   overridden and did not use the default `Set` implementation. So, duplicates were still
    ///   silently ignored (while still allowing a set tag, and still expecting at least one
    ///   element):
    ///
    ///   <https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L610-L624>
    ///
    ///   Importantly, this behaviour is changing again in v12, back to being a non-empty set.
    #[n(0)]
    pub vkeywitness: Option<NonEmptyVec<VKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptySet<MemoizedNativeScript>>,

    /// FIXME: Accidentally not a set
    ///
    /// See note on vkeywitness.
    #[n(2)]
    pub bootstrap_witness: Option<NonEmptyVec<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptySet<PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<NonEmptySet<MemoizedPlutusData>>,

    #[n(5)]
    pub redeemer: Option<Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptySet<PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptySet<PlutusScript<3>>>,
}

impl WitnessSet {
    /// Collect provided scripts and compute each ScriptHash in a witness set
    pub fn get_provided_scripts(&self) -> BTreeMap<Hash<SCRIPT>, ScriptKind> {
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
    accum: &mut BTreeMap<Hash<SCRIPT>, ScriptKind>,
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
