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

use crate::{cbor, size::KEY, Hash, ValidityInterval};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NativeScript {
    ScriptPubkey(Hash<{ KEY }>),
    ScriptAll(Vec<NativeScript>),
    ScriptAny(Vec<NativeScript>),
    ScriptNOfK(i64, Vec<NativeScript>),
    InvalidBefore(u64),
    InvalidHereafter(u64),
}

// TODO: NativeScript vs MemoizedNativeScript
//
// This instance should not exist / be public. We shoul only authorize decoding
// MemoizedNativeScript. Ideally, there shouldn't even be two types.
impl<'b, C> cbor::decode::Decode<'b, C> for NativeScript {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            let variant = d.u32()?;

            match variant {
                0 => {
                    assert_len(2)?;
                    Ok(NativeScript::ScriptPubkey(d.decode_with(ctx)?))
                }
                1 => {
                    assert_len(2)?;
                    Ok(NativeScript::ScriptAll(d.decode_with(ctx)?))
                }
                2 => {
                    assert_len(2)?;
                    Ok(NativeScript::ScriptAny(d.decode_with(ctx)?))
                }
                3 => {
                    assert_len(3)?;
                    Ok(NativeScript::ScriptNOfK(d.decode_with(ctx)?, d.decode_with(ctx)?))
                }
                4 => {
                    assert_len(2)?;
                    Ok(NativeScript::InvalidBefore(d.decode_with(ctx)?))
                }
                5 => {
                    assert_len(2)?;
                    Ok(NativeScript::InvalidHereafter(d.decode_with(ctx)?))
                }
                _ => Err(cbor::decode::Error::message("unknown variant id for native script")),
            }
        })
    }
}

impl<C> cbor::encode::Encode<C> for NativeScript {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            NativeScript::ScriptPubkey(v) => {
                e.array(2)?;
                e.encode_with(0, ctx)?;
                e.encode_with(v, ctx)?;
            }
            NativeScript::ScriptAll(v) => {
                e.array(2)?;
                e.encode_with(1, ctx)?;
                e.encode_with(v, ctx)?;
            }
            NativeScript::ScriptAny(v) => {
                e.array(2)?;
                e.encode_with(2, ctx)?;
                e.encode_with(v, ctx)?;
            }
            NativeScript::ScriptNOfK(a, b) => {
                e.array(3)?;
                e.encode_with(3, ctx)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
            }
            NativeScript::InvalidBefore(v) => {
                e.array(2)?;
                e.encode_with(4, ctx)?;
                e.encode_with(v, ctx)?;
            }
            NativeScript::InvalidHereafter(v) => {
                e.array(2)?;
                e.encode_with(5, ctx)?;
                e.encode_with(v, ctx)?;
            }
        }

        Ok(())
    }
}

impl NativeScript {
    /// Evaluate a native script against a set of required signer key hashes and a transaction validity interval.
    pub fn eval(&self, vkey_hashes: &BTreeSet<Hash<KEY>>, validity_interval: ValidityInterval) -> bool {
        match self {
            Self::ScriptPubkey(key) => vkey_hashes.contains(key),
            Self::ScriptAll(scripts) => scripts.iter().all(|s| s.eval(vkey_hashes, validity_interval)),
            Self::ScriptAny(scripts) => scripts.iter().any(|s| s.eval(vkey_hashes, validity_interval)),
            // NOTE: Laziness of ScriptNOfK
            //
            // The NOfK scripts are evaluated lazily, stopping once we have n scripts that evaluate to
            // true. The test `iter_filter_take_evaluates_lazily` illustrates this behavior.
            Self::ScriptNOfK(n, scripts) => {
                // A non-positive threshold is trivially satisfied, matching the ledger's `m <= satisfied`.
                let n = (*n).max(0) as usize;
                scripts.iter().filter(|s| s.eval(vkey_hashes, validity_interval)).take(n).count() == n
            }
            // `lteNegInfty`: a lock requiring `lock_start <= ValidityInterval::lower_bound()` can only be satisfied when
            // `tx_start` is given. A missing lower bound is treated as -inf and always fails.
            Self::InvalidBefore(lock_start) => {
                validity_interval.lower_bound().is_some_and(|t| lock_start <= &t.as_u64())
            }
            // `ltePosInfty`: a lock requiring `ValidityInterval::upper_bound() <= lock_expire` can only be satisfied when
            // `tx_expire` is given. A missing upper bound is treated as +inf and always fails.
            Self::InvalidHereafter(lock_expire) => {
                validity_interval.upper_bound().is_some_and(|t| &t.as_u64() <= lock_expire)
            }
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::NativeScript;
    use crate::any_hash28;

    // --------------------------------------------------------------------------------------------
    // Generators
    // --------------------------------------------------------------------------------------------

    pub fn any_native_script(depth: u8) -> BoxedStrategy<NativeScript> {
        use NativeScript::*;

        let sig = any_hash28().prop_map(ScriptPubkey);
        let before = any::<u64>().prop_map(InvalidBefore);
        let after = any::<u64>().prop_map(InvalidHereafter);

        if depth > 0 {
            let all = prop::collection::vec(any_native_script(depth - 1), 0..depth as usize).prop_map(ScriptAll);

            let some = prop::collection::vec(any_native_script(depth - 1), 0..depth as usize).prop_map(ScriptAny);

            let n_of_k = (any::<i64>(), prop::collection::vec(any_native_script(depth - 1), 0..depth as usize))
                .prop_map(|(n, sigs)| ScriptNOfK(n, sigs));

            prop_oneof![sig, before, after, all, some, n_of_k,].boxed()
        } else {
            prop_oneof![sig, before, after].boxed()
        }
    }

    // --------------------------------------------------------------------------------------------
    // Tests
    // --------------------------------------------------------------------------------------------

    #[cfg(test)]
    mod internal {
        use std::collections::BTreeSet;

        use test_case::test_case;

        use crate::{size::KEY, Hash, NativeScript, NativeScript::*, ValidityInterval};

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
            assert!(script.eval(&context_vkey_hashes(context_keys), validity_interval));
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
            assert!(!script.eval(&context_vkey_hashes(context_keys), validity_interval));
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

        fn at_least<const N: usize>(n: i64, scripts: [NativeScript; N]) -> NativeScript {
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
}
