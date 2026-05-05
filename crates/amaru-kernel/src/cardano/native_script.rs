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

    use super::evaluate_native_script;
    use crate::{Hash, NativeScript::*, ValidityInterval, size::KEY};

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

    fn key(byte: u8) -> Hash<KEY> {
        Hash::from([byte; 28])
    }

    fn keys(bytes: &[u8]) -> BTreeSet<Hash<KEY>> {
        bytes.iter().copied().map(key).collect()
    }

    #[test]
    fn script_pubkey_present() {
        let script = ScriptPubkey(key(1));
        assert!(evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_pubkey_absent() {
        let script = ScriptPubkey(key(3));
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_all_all_pass() {
        let script = ScriptAll(vec![ScriptPubkey(key(1)), ScriptPubkey(key(2))]);
        assert!(evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_all_one_fails() {
        let script = ScriptAll(vec![ScriptPubkey(key(1)), ScriptPubkey(key(3))]);
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_all_empty_is_true() {
        let script = ScriptAll(vec![]);
        assert!(evaluate_native_script(&script, &keys(&[]), ValidityInterval::default()));
    }

    #[test]
    fn script_any_one_passes() {
        let script = ScriptAny(vec![ScriptPubkey(key(3)), ScriptPubkey(key(1))]);
        assert!(evaluate_native_script(&script, &keys(&[1]), ValidityInterval::default()));
    }

    #[test]
    fn script_any_all_fail() {
        let script = ScriptAny(vec![ScriptPubkey(key(3)), ScriptPubkey(key(4))]);
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_any_empty_is_false() {
        let script = ScriptAny(vec![]);
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_n_of_k_zero_always_passes() {
        let script = ScriptNOfK(0, vec![ScriptPubkey(key(9))]);
        assert!(evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_n_of_k_exact_quorum() {
        let script = ScriptNOfK(2, vec![ScriptPubkey(key(1)), ScriptPubkey(key(2)), ScriptPubkey(key(9))]);
        assert!(evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_n_of_k_just_below_quorum() {
        let script = ScriptNOfK(2, vec![ScriptPubkey(key(1)), ScriptPubkey(key(8)), ScriptPubkey(key(9))]);
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn script_n_of_k_more_than_available() {
        let script = ScriptNOfK(3, vec![ScriptPubkey(key(1)), ScriptPubkey(key(2))]);
        assert!(!evaluate_native_script(&script, &keys(&[1, 2]), ValidityInterval::default()));
    }

    #[test]
    fn invalid_before_with_tx_start_ge_lock() {
        let script = InvalidBefore(100);
        assert!(evaluate_native_script(&script, &keys(&[]), ValidityInterval::after(100.into())));
        assert!(evaluate_native_script(&script, &keys(&[]), ValidityInterval::after(101.into())));
    }

    #[test]
    fn invalid_before_with_tx_start_below_lock() {
        let script = InvalidBefore(100);
        assert!(!evaluate_native_script(&script, &keys(&[]), ValidityInterval::after(99.into())));
    }

    #[test]
    fn invalid_before_without_tx_start() {
        let script = InvalidBefore(100);
        assert!(!evaluate_native_script(&script, &keys(&[]), ValidityInterval::default()));
    }

    #[test]
    fn invalid_hereafter_with_tx_expire_le_lock() {
        let script = InvalidHereafter(100);
        assert!(evaluate_native_script(&script, &keys(&[]), ValidityInterval::strictly_before(100.into())));
        assert!(evaluate_native_script(&script, &keys(&[]), ValidityInterval::strictly_before(50.into())));
    }

    #[test]
    fn invalid_hereafter_with_tx_expire_above_lock() {
        let script = InvalidHereafter(100);
        assert!(!evaluate_native_script(&script, &keys(&[]), ValidityInterval::strictly_before(101.into())));
    }

    #[test]
    fn invalid_hereafter_without_tx_expire() {
        let script = InvalidHereafter(100);
        assert!(!evaluate_native_script(&script, &keys(&[]), ValidityInterval::default()));
    }

    #[test]
    fn nested_all_of_any_of_and_timelock() {
        let script = ScriptAll(vec![
            ScriptAny(vec![ScriptPubkey(key(8)), ScriptPubkey(key(1))]),
            InvalidBefore(100),
            InvalidHereafter(200),
        ]);
        assert!(evaluate_native_script(&script, &keys(&[1]), ValidityInterval::between(150.into(), 199.into())));
        assert!(!evaluate_native_script(&script, &keys(&[1]), ValidityInterval::between(99.into(), 199.into())));
        assert!(!evaluate_native_script(&script, &keys(&[1]), ValidityInterval::between(150.into(), 201.into())));
        assert!(!evaluate_native_script(&script, &keys(&[9]), ValidityInterval::between(150.into(), 199.into())));
    }
}
