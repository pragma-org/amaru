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
    Hash, Hasher, NativeScript, WitnessSet,
    size::{KEY, SCRIPT},
};

use super::scripts::{InvalidScripts, ProvidedScript};

/// Evaluate a native script against a set of required signer key hashes and a transaction validity interval.
pub fn evaluate(
    script: &NativeScript,
    vkey_hashes: &BTreeSet<Hash<KEY>>,
    tx_start: Option<u64>,
    tx_expire: Option<u64>,
) -> bool {
    match script {
        NativeScript::ScriptPubkey(key) => vkey_hashes.contains(key),
        NativeScript::ScriptAll(scripts) => scripts.iter().all(|s| evaluate(s, vkey_hashes, tx_start, tx_expire)),
        NativeScript::ScriptAny(scripts) => scripts.iter().any(|s| evaluate(s, vkey_hashes, tx_start, tx_expire)),
        NativeScript::ScriptNOfK(m, scripts) => {
            let m = *m as usize;
            scripts.iter().filter(|s| evaluate(s, vkey_hashes, tx_start, tx_expire)).take(m).count() == m
        }
        // `lteNegInfty`: a lock requiring `lock_start <= tx_start` can only be satisfied when
        // `tx_start` is given. A missing lower bound is treated as -inf and always fails.
        NativeScript::InvalidBefore(lock_start) => matches!(tx_start, Some(t) if *lock_start <= t),
        // `ltePosInfty`: a lock requiring `tx_expire <= lock_expire` can only be satisfied when
        // `tx_expire` is given. A missing upper bound is treated as +inf and always fails.
        NativeScript::InvalidHereafter(lock_expire) => matches!(tx_expire, Some(t) if t <= *lock_expire),
    }
}

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
    tx_start: Option<u64>,
    tx_expire: Option<u64>,
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
        .filter_map(|(hash, script)| (!evaluate(script, &vkey_hashes, tx_start, tx_expire)).then_some(*hash))
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

#[cfg(test)]
mod tests {
    use amaru_kernel::{Hash, NativeScript};

    use super::*;

    fn k(byte: u8) -> Hash<KEY> {
        Hash::from([byte; 28])
    }

    fn keys(bytes: &[u8]) -> BTreeSet<Hash<KEY>> {
        bytes.iter().copied().map(k).collect()
    }

    #[test]
    fn script_pubkey_present() {
        let script = NativeScript::ScriptPubkey(k(1));
        assert!(evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_pubkey_absent() {
        let script = NativeScript::ScriptPubkey(k(3));
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_all_all_pass() {
        let script = NativeScript::ScriptAll(vec![NativeScript::ScriptPubkey(k(1)), NativeScript::ScriptPubkey(k(2))]);
        assert!(evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_all_one_fails() {
        let script = NativeScript::ScriptAll(vec![NativeScript::ScriptPubkey(k(1)), NativeScript::ScriptPubkey(k(3))]);
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_all_empty_is_true() {
        let script = NativeScript::ScriptAll(vec![]);
        assert!(evaluate(&script, &keys(&[]), None, None));
    }

    #[test]
    fn script_any_one_passes() {
        let script = NativeScript::ScriptAny(vec![NativeScript::ScriptPubkey(k(3)), NativeScript::ScriptPubkey(k(1))]);
        assert!(evaluate(&script, &keys(&[1]), None, None));
    }

    #[test]
    fn script_any_all_fail() {
        let script = NativeScript::ScriptAny(vec![NativeScript::ScriptPubkey(k(3)), NativeScript::ScriptPubkey(k(4))]);
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_any_empty_is_false() {
        let script = NativeScript::ScriptAny(vec![]);
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_n_of_k_zero_always_passes() {
        let script = NativeScript::ScriptNOfK(0, vec![NativeScript::ScriptPubkey(k(9))]);
        assert!(evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_n_of_k_exact_quorum() {
        let script = NativeScript::ScriptNOfK(
            2,
            vec![NativeScript::ScriptPubkey(k(1)), NativeScript::ScriptPubkey(k(2)), NativeScript::ScriptPubkey(k(9))],
        );
        assert!(evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_n_of_k_just_below_quorum() {
        let script = NativeScript::ScriptNOfK(
            2,
            vec![NativeScript::ScriptPubkey(k(1)), NativeScript::ScriptPubkey(k(8)), NativeScript::ScriptPubkey(k(9))],
        );
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn script_n_of_k_more_than_available() {
        let script =
            NativeScript::ScriptNOfK(3, vec![NativeScript::ScriptPubkey(k(1)), NativeScript::ScriptPubkey(k(2))]);
        assert!(!evaluate(&script, &keys(&[1, 2]), None, None));
    }

    #[test]
    fn invalid_before_with_tx_start_ge_lock() {
        let script = NativeScript::InvalidBefore(100);
        assert!(evaluate(&script, &keys(&[]), Some(100), None));
        assert!(evaluate(&script, &keys(&[]), Some(101), None));
    }

    #[test]
    fn invalid_before_with_tx_start_below_lock() {
        let script = NativeScript::InvalidBefore(100);
        assert!(!evaluate(&script, &keys(&[]), Some(99), None));
    }

    #[test]
    fn invalid_before_without_tx_start() {
        let script = NativeScript::InvalidBefore(100);
        assert!(!evaluate(&script, &keys(&[]), None, None));
    }

    #[test]
    fn invalid_hereafter_with_tx_expire_le_lock() {
        let script = NativeScript::InvalidHereafter(100);
        assert!(evaluate(&script, &keys(&[]), None, Some(100)));
        assert!(evaluate(&script, &keys(&[]), None, Some(50)));
    }

    #[test]
    fn invalid_hereafter_with_tx_expire_above_lock() {
        let script = NativeScript::InvalidHereafter(100);
        assert!(!evaluate(&script, &keys(&[]), None, Some(101)));
    }

    #[test]
    fn invalid_hereafter_without_tx_expire() {
        let script = NativeScript::InvalidHereafter(100);
        assert!(!evaluate(&script, &keys(&[]), None, None));
    }

    #[test]
    fn nested_all_of_any_of_and_timelock() {
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptAny(vec![NativeScript::ScriptPubkey(k(8)), NativeScript::ScriptPubkey(k(1))]),
            NativeScript::InvalidBefore(100),
            NativeScript::InvalidHereafter(200),
        ]);
        assert!(evaluate(&script, &keys(&[1]), Some(150), Some(199)));
        assert!(!evaluate(&script, &keys(&[1]), Some(99), Some(199)));
        assert!(!evaluate(&script, &keys(&[1]), Some(150), Some(201)));
        assert!(!evaluate(&script, &keys(&[9]), Some(150), Some(199)));
    }
}
