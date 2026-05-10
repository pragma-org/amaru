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

use std::collections::BTreeSet;

pub use pallas_primitives::conway::NativeScript;

use crate::{Hash, ValidityInterval, size::KEY};

/// Evaluate a native script against a set of required signer key hashes and a transaction validity interval.
pub fn evaluate_native_script(
    script: &NativeScript,
    vkey_hashes: &BTreeSet<Hash<KEY>>,
    validity_interval: ValidityInterval,
) -> bool {
    match script {
        NativeScript::ScriptPubkey(key) => vkey_hashes.contains(key),
        NativeScript::ScriptAll(scripts) => {
            scripts.iter().all(|s| evaluate_native_script(s, vkey_hashes, validity_interval))
        }
        NativeScript::ScriptAny(scripts) => {
            scripts.iter().any(|s| evaluate_native_script(s, vkey_hashes, validity_interval))
        }
        // NOTE: Laziness of ScriptNOfK
        //
        // The NOfK scripts are evaluated lazily, stopping once we have n scripts that evaluate to
        // true. The test `iter_filter_take_evaluates_lazily` illustrates this behavior.
        NativeScript::ScriptNOfK(n, scripts) => {
            let n = *n as usize;
            scripts.iter().filter(|s| evaluate_native_script(s, vkey_hashes, validity_interval)).take(n).count() == n
        }
        // `lteNegInfty`: a lock requiring `lock_start <= ValidityInterval::lower_bound()` can only be satisfied when
        // `tx_start` is given. A missing lower bound is treated as -inf and always fails.
        NativeScript::InvalidBefore(lock_start) => {
            validity_interval.lower_bound().is_some_and(|t| lock_start <= &t.as_u64())
        }
        // `ltePosInfty`: a lock requiring `ValidityInterval::upper_bound() <= lock_expire` can only be satisfied when
        // `tx_expire` is given. A missing upper bound is treated as +inf and always fails.
        NativeScript::InvalidHereafter(lock_expire) => {
            validity_interval.upper_bound().is_some_and(|t| &t.as_u64() <= lock_expire)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use test_case::test_case;

    use super::evaluate_native_script;
    use crate::{Hash, NativeScript, NativeScript::*, ValidityInterval, size::KEY};

    /// The following test proves that the scriptNOfK evaluate_native_scripts native scripts lazily.
    /// If they weren't, this test would panic.
    ///
    /// This test is intentionally left out of the test suite, as it's testing the behavior of the stdlib.
    /// However, it is left here so anyone can choose to run it locally if they want proof of the above statement.
    #[test]
    fn iter_filter_take_evaluates_lazily() {
        let scripts: Vec<Box<dyn Fn() -> bool>> = vec![
            Box::new(|| true),
            Box::new(|| true),
            Box::new(|| true),
            Box::new(|| panic!("must not be evaluated after quorum is reached")),
            Box::new(|| panic!("must not be evaluated after quorum is reached")),
        ];

        let n = 3usize;

        assert_eq!(scripts.iter().filter(|s| s()).take(n).count(), n);
    }

    #[test_case(vk(1), &[vk(1), vk(2)], always(); "script pubkey present")]
    #[test_case(all([vk(1), vk(2)]), &[vk(1), vk(2)], always(); "script all all pass")]
    #[test_case(all([]), &[], always(); "script all empty is true")]
    #[test_case(any([vk(3), vk(1)]), &[vk(1)], always(); "script any one passes")]
    #[test_case(at_least(0, [vk(9)]), &[vk(1), vk(2)], always(); "script n of k zero always passes")]
    #[test_case(at_least(2, [vk(1), vk(2), vk(9)]), &[vk(1), vk(2)], always(); "script n of k exact quorum")]
    #[test_case(InvalidBefore(100), &[], after(100); "invalid before with tx start at lock")]
    #[test_case(InvalidBefore(100), &[], after(101); "invalid before with tx start above lock")]
    #[test_case(InvalidHereafter(100), &[], before(100); "invalid hereafter with tx expire at lock")]
    #[test_case(InvalidHereafter(100), &[], before(50); "invalid hereafter with tx expire below lock")]
    #[test_case(
        all([any([vk(8), vk(1)]), InvalidBefore(100), InvalidHereafter(200)]),
        &[vk(1)],
        between(150, 199);
        "nested all any timelock all conditions pass"
    )]
    fn ok(script: NativeScript, context_keys: &[NativeScript], validity_interval: ValidityInterval) {
        assert!(evaluate_native_script(&script, &context_vkey_hashes(context_keys), validity_interval));
    }

    #[test_case(vk(3), &[vk(1), vk(2)], always(); "script pubkey absent")]
    #[test_case(all([vk(1), vk(3)]), &[vk(1), vk(2)], always(); "script all one fails")]
    #[test_case(any([vk(3), vk(4)]), &[vk(1), vk(2)], always(); "script any all fail")]
    #[test_case(any([]), &[vk(1), vk(2)], always(); "script any empty is false")]
    #[test_case(at_least(2, [vk(1), vk(8), vk(9)]), &[vk(1), vk(2)], always(); "script n of k just below quorum")]
    #[test_case(at_least(3, [vk(1), vk(2)]), &[vk(1), vk(2)], always(); "script n of k more than available")]
    #[test_case(InvalidBefore(100), &[], after(99); "invalid before with tx start below lock")]
    #[test_case(InvalidBefore(100), &[], always(); "invalid before without tx start")]
    #[test_case(InvalidHereafter(100), &[], before(101); "invalid hereafter with tx expire above lock")]
    #[test_case(InvalidHereafter(100), &[], always(); "invalid hereafter without tx expire")]
    #[test_case(
        all([any([vk(8), vk(1)]), InvalidBefore(100), InvalidHereafter(200)]),
        &[vk(1)],
        between(99, 199);
        "nested all any timelock lower bound fails"
    )]
    #[test_case(
        all([any([vk(8), vk(1)]), InvalidBefore(100), InvalidHereafter(200)]),
        &[vk(1)],
        between(150, 201);
        "nested all any timelock upper bound fails"
    )]
    #[test_case(
        all([any([vk(8), vk(1)]), InvalidBefore(100), InvalidHereafter(200)]),
        &[vk(9)],
        between(150, 199);
        "nested all any timelock key check fails"
    )]
    fn ko(script: NativeScript, context_keys: &[NativeScript], validity_interval: ValidityInterval) {
        assert!(!evaluate_native_script(&script, &context_vkey_hashes(context_keys), validity_interval));
    }

    // ------------------------------------------------------------------------ Helpers

    fn vk(byte: u8) -> NativeScript {
        ScriptPubkey(Hash::from([byte; 28]))
    }

    fn all<const N: usize>(scripts: [NativeScript; N]) -> NativeScript {
        ScriptAll(scripts.into())
    }

    fn any<const N: usize>(scripts: [NativeScript; N]) -> NativeScript {
        ScriptAny(scripts.into())
    }

    fn at_least<const N: usize>(n: u32, scripts: [NativeScript; N]) -> NativeScript {
        ScriptNOfK(n, scripts.into())
    }

    fn always() -> ValidityInterval {
        ValidityInterval::default()
    }

    fn after(slot: u64) -> ValidityInterval {
        ValidityInterval::after(slot.into())
    }

    fn before(slot: u64) -> ValidityInterval {
        ValidityInterval::strictly_before(slot.into())
    }

    fn between(lower_bound: u64, upper_bound: u64) -> ValidityInterval {
        ValidityInterval::between(lower_bound.into(), upper_bound.into())
    }

    #[allow(clippy::wildcard_enum_match_arm)]
    fn context_vkey_hashes(context_keys: &[NativeScript]) -> BTreeSet<Hash<KEY>> {
        context_keys
            .iter()
            .map(|script| match script {
                ScriptPubkey(hash) => *hash,
                _ => panic!("expected ScriptPubkey in validation context"),
            })
            .collect()
    }
}
