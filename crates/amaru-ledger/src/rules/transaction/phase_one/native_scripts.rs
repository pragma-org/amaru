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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Hash, Hasher, NativeScript, ValidityInterval, WitnessSet, evaluate_native_script,
    size::{KEY, SCRIPT},
};

use super::scripts::{InvalidScripts, ProvidedScript};

/// Check that every required native script provided by the transaction actually validates.
///
/// Mirrors [`validateFailedBabbageScripts`][haskell] in `cardano-ledger`: filter the provided
/// scripts down to native scripts that are needed and do not validate. A non-empty result is a
/// `ScriptWitnessNotValidatingUTXOW` failure.
///
/// [haskell]: https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/babbage/impl/src/Cardano/Ledger/Babbage/Rules/Utxow.hs#L226
pub(super) fn execute(
    provided_scripts: &BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>>,
    required_script_hashes: &BTreeSet<&Hash<SCRIPT>>,
    witness_set: &WitnessSet,
    validity_interval: ValidityInterval,
) -> Result<(), InvalidScripts> {
    let required_natives: Vec<(&Hash<SCRIPT>, &NativeScript)> = required_script_hashes
        .iter()
        .filter_map(|hash| match provided_scripts.get(*hash)? {
            ProvidedScript::Native(script) => Some((*hash, *script)),
            ProvidedScript::PlutusV1 | ProvidedScript::PlutusV2 | ProvidedScript::PlutusV3 => None,
        })
        .collect();

    if required_natives.is_empty() {
        return Ok(());
    }

    let vkey_hashes = collect_vkey_hashes(witness_set);

    let failing: BTreeSet<Hash<SCRIPT>> = required_natives
        .into_iter()
        .filter_map(|(hash, script)| {
            (!evaluate_native_script(script, &vkey_hashes, validity_interval)).then_some(*hash)
        })
        .collect();

    if !failing.is_empty() {
        return Err(InvalidScripts::ScriptWitnessNotValidatingUTXOW(failing));
    }

    Ok(())
}

// FIXME: This duplicates the vkey hashing already done in
// `vkey_witness::execute`, which builds a `provided_keys_or_roots` set from the same vkey
// witnesses. However, that set is the UNION of vkey hashes and bootstrap-witness roots.
// Native script `RequireSignature` evaluation must NOT consider bootstrap roots.
//
// Updating the context to either split the provided_keys_or_roots into two different sets or
// threading the vkey hashes throuhg phase one validation are both larger changes that could delay progress.
// The hashing 32 bytes is not a significant increase in valdiation time, so we will leave this for later
fn collect_vkey_hashes(witness_set: &WitnessSet) -> BTreeSet<Hash<KEY>> {
    witness_set.vkeywitness.as_deref().unwrap_or(&[]).iter().map(|witness| Hasher::<224>::hash(&witness.vkey)).collect()
}
