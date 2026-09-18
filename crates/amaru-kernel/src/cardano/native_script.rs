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

use std::{collections::BTreeSet, ops::Deref};

use stacksafe::{StackSafe, stacksafe};

use crate::{Hash, ValidityInterval, cbor, size::KEY, utils::string::blanket_try_from_hex_bytes};

// -------------------------------------------------------------------------------------------------
// ------------------------------------------------------------------------------------ NativeScript
// -------------------------------------------------------------------------------------------------

/// A native script, held as the bytes it was decoded from together with its nodes in pre-order,
/// which is the order those bytes lay them out in.
///
/// A script hash is taken over the bytes, and definite and indefinite encodings of the same tree
/// hash differently, so a script keeps the bytes it arrived with instead of re-deriving them.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "String")]
pub struct NativeScript {
    original_bytes: Vec<u8>,
    tree: NativeScriptTree,
}

impl NativeScript {
    #[cfg(any(test, feature = "test-utils"))]
    fn new(tree: NativeScriptTree) -> Self {
        Self { original_bytes: crate::to_cbor(&tree), tree }
    }

    pub fn eval(&self, verification_key_hashes: &BTreeSet<Hash<KEY>>, validity_interval: ValidityInterval) -> bool {
        self.tree.eval(verification_key_hashes, validity_interval)
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl Eq for NativeScript {}
impl PartialEq for NativeScript {
    fn eq(&self, rhs: &NativeScript) -> bool {
        self.tree.eq(&rhs.tree)
    }
}

impl serde::Serialize for NativeScript {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.original_bytes))
    }
}

impl TryFrom<&str> for NativeScript {
    type Error = String;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, tree| Self { original_bytes, tree })
    }
}

impl TryFrom<String> for NativeScript {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for NativeScript {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (tree, original_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;
        Ok(Self { original_bytes: original_bytes.to_vec(), tree })
    }
}

impl<C> cbor::Encode<C> for NativeScript {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.original_bytes[..]).map_err(cbor::encode::Error::write)
    }
}

// -------------------------------------------------------------------------------------------------
// -------------------------------------------------------------------------------- NativeScriptTree
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeScriptTree {
    VerificationKey(Hash<{ KEY }>),
    All(StackSafe<Vec<NativeScriptTree>>),
    Any(StackSafe<Vec<NativeScriptTree>>),
    AtLeast(i64, StackSafe<Vec<NativeScriptTree>>),
    InvalidBefore(u64),
    InvalidAfter(u64),
}

impl NativeScriptTree {
    #[cfg(test)]
    pub fn verification_key(hash: Hash<{ KEY }>) -> Self {
        Self::VerificationKey(hash)
    }

    #[cfg(test)]
    pub fn all(scripts: Vec<Self>) -> Self {
        Self::All(scripts.into())
    }

    #[cfg(test)]
    pub fn any(scripts: Vec<Self>) -> Self {
        Self::Any(scripts.into())
    }

    #[cfg(test)]
    pub fn at_least(n: i64, scripts: Vec<Self>) -> Self {
        Self::AtLeast(n, scripts.into())
    }

    #[cfg(test)]
    pub fn invalid_before(slot: u64) -> Self {
        Self::InvalidBefore(slot)
    }

    #[cfg(test)]
    pub fn invalid_after(slot: u64) -> Self {
        Self::InvalidAfter(slot)
    }

    /// Evaluate a native script against a set of required signer key hashes and a transaction validity interval.
    #[stacksafe]
    pub fn eval(&self, verification_key_hashes: &BTreeSet<Hash<KEY>>, validity_interval: ValidityInterval) -> bool {
        match self {
            Self::VerificationKey(key) => verification_key_hashes.contains(key),
            Self::All(scripts) => scripts.iter().all(|s| s.eval(verification_key_hashes, validity_interval)),
            Self::Any(scripts) => scripts.iter().any(|s| s.eval(verification_key_hashes, validity_interval)),
            // NOTE: Laziness of ScriptNOfK
            //
            // The AtLeast scripts are evaluated lazily, stopping once we have n scripts that evaluate to
            // true. The test `iter_filter_take_evaluates_lazily` illustrates this behavior.
            Self::AtLeast(n, scripts) => {
                // A non-positive threshold is trivially satisfied, matching the ledger's `m <= satisfied`.
                let n = (*n).max(0) as usize;
                scripts.iter().filter(|s| s.eval(verification_key_hashes, validity_interval)).take(n).count() == n
            }
            // `lteNegInfty`: a lock requiring `lock_start <= ValidityInterval::lower_bound()` can only be satisfied when
            // `tx_start` is given. A missing lower bound is treated as -inf and always fails.
            Self::InvalidBefore(lock_start) => {
                validity_interval.lower_bound().is_some_and(|t| lock_start <= &t.as_u64())
            }
            // `ltePosInfty`: a lock requiring `ValidityInterval::upper_bound() <= lock_expire` can only be satisfied when
            // `tx_expire` is given. A missing upper bound is treated as +inf and always fails.
            Self::InvalidAfter(lock_expire) => {
                validity_interval.upper_bound().is_some_and(|t| &t.as_u64() <= lock_expire)
            }
        }
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for NativeScriptTree {
    #[stacksafe]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| match d.u32()? {
            0 => {
                assert_len(2)?;
                Ok(Self::VerificationKey(d.decode_with(ctx)?))
            }
            1 => {
                assert_len(2)?;
                Ok(Self::All(StackSafe::new(d.decode_with(ctx)?)))
            }
            2 => {
                assert_len(2)?;
                Ok(Self::Any(StackSafe::new(d.decode_with(ctx)?)))
            }
            3 => {
                assert_len(3)?;
                Ok(Self::AtLeast(d.decode_with(ctx)?, StackSafe::new(d.decode_with(ctx)?)))
            }
            4 => {
                assert_len(2)?;
                Ok(Self::InvalidBefore(d.decode_with(ctx)?))
            }
            5 => {
                assert_len(2)?;
                Ok(Self::InvalidAfter(d.decode_with(ctx)?))
            }
            _ => Err(cbor::decode::Error::message("unexpected variant id for native script")),
        })
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for NativeScriptTree {
    #[stacksafe]
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::VerificationKey(v) => {
                e.array(2)?;
                e.encode_with(0, ctx)?;
                e.encode_with(v, ctx)?;
            }
            Self::All(v) => {
                e.array(2)?;
                e.encode_with(1, ctx)?;
                e.encode_with(v.deref(), ctx)?;
            }
            Self::Any(v) => {
                e.array(2)?;
                e.encode_with(2, ctx)?;
                e.encode_with(v.deref(), ctx)?;
            }
            Self::AtLeast(n, v) => {
                e.array(3)?;
                e.encode_with(3, ctx)?;
                e.encode_with(n, ctx)?;
                e.encode_with(v.deref(), ctx)?;
            }
            Self::InvalidBefore(v) => {
                e.array(2)?;
                e.encode_with(4, ctx)?;
                e.encode_with(v, ctx)?;
            }
            Self::InvalidAfter(v) => {
                e.array(2)?;
                e.encode_with(5, ctx)?;
                e.encode_with(v, ctx)?;
            }
        }

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(test)]
mod variable_encoding_native_script {
    use proptest::prelude::*;

    use super::NativeScriptTree;
    use crate::{Hash, NativeScript, any_hash28, cbor, size::KEY, to_cbor, utils::cbor::CborArray};

    /// A native script that also picks, at every branch, whether to encode its children as a
    /// definite or an indefinite array.
    #[derive(Debug, Clone)]
    pub enum VariableEncodingNativeScript {
        ScriptPubkey(Hash<KEY>),
        ScriptAll(CborArray<VariableEncodingNativeScript>),
        ScriptAny(CborArray<VariableEncodingNativeScript>),
        ScriptNOfK(i64, CborArray<VariableEncodingNativeScript>),
        InvalidBefore(u64),
        InvalidHereafter(u64),
    }

    impl VariableEncodingNativeScript {
        const MAX_BREADTH: usize = 3;

        pub fn any(depth: u8) -> BoxedStrategy<Self> {
            use VariableEncodingNativeScript::*;

            let sig = any_hash28().prop_map(ScriptPubkey);
            let before = any::<u64>().prop_map(InvalidBefore);
            let after = any::<u64>().prop_map(InvalidHereafter);

            if depth > 0 {
                let all = Self::children(depth).prop_map(ScriptAll);
                let some = Self::children(depth).prop_map(ScriptAny);
                let n_of_k = (any::<i64>(), Self::children(depth)).prop_map(|(n, scripts)| ScriptNOfK(n, scripts));

                prop_oneof![sig, before, after, all, some, n_of_k,].boxed()
            } else {
                prop_oneof![sig, before, after].boxed()
            }
        }

        fn children(depth: u8) -> impl Strategy<Value = CborArray<Self>> {
            (any::<bool>(), prop::collection::vec(Self::any(depth - 1), 0..Self::MAX_BREADTH)).prop_map(
                |(is_definite, scripts)| {
                    if is_definite { CborArray::Def(scripts) } else { CborArray::Indef(scripts) }
                },
            )
        }
    }

    impl From<VariableEncodingNativeScript> for NativeScript {
        fn from(script: VariableEncodingNativeScript) -> Self {
            Self::new(NativeScriptTree::from(script))
        }
    }

    impl From<VariableEncodingNativeScript> for NativeScriptTree {
        fn from(script: VariableEncodingNativeScript) -> Self {
            use VariableEncodingNativeScript::*;

            fn from_vec(scripts: CborArray<VariableEncodingNativeScript>) -> Vec<NativeScriptTree> {
                Vec::from(scripts).into_iter().map(NativeScriptTree::from).collect()
            }

            match script {
                ScriptPubkey(sig) => NativeScriptTree::verification_key(sig),
                ScriptAll(scripts) => NativeScriptTree::all(from_vec(scripts)),
                ScriptAny(scripts) => NativeScriptTree::any(from_vec(scripts)),
                ScriptNOfK(n, scripts) => NativeScriptTree::at_least(n, from_vec(scripts)),
                InvalidBefore(n) => NativeScriptTree::invalid_before(n),
                InvalidHereafter(n) => NativeScriptTree::invalid_after(n),
            }
        }
    }

    impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for VariableEncodingNativeScript {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            match self {
                Self::ScriptPubkey(sig) => {
                    e.array(2)?;
                    e.encode_with(0, ctx)?;
                    e.encode_with(sig, ctx)?;
                }
                Self::ScriptAll(sigs) => {
                    e.array(2)?;
                    e.encode_with(1, ctx)?;
                    e.encode_with(sigs, ctx)?;
                }
                Self::ScriptAny(sigs) => {
                    e.array(2)?;
                    e.encode_with(2, ctx)?;
                    e.encode_with(sigs, ctx)?;
                }
                Self::ScriptNOfK(n, sigs) => {
                    e.array(3)?;
                    e.encode_with(3, ctx)?;
                    e.encode_with(n, ctx)?;
                    e.encode_with(sigs, ctx)?;
                }
                Self::InvalidBefore(n) => {
                    e.array(2)?;
                    e.encode_with(4, ctx)?;
                    e.encode_with(n, ctx)?;
                }
                Self::InvalidHereafter(n) => {
                    e.array(2)?;
                    e.encode_with(5, ctx)?;
                    e.encode_with(n, ctx)?;
                }
            };

            Ok(())
        }
    }

    proptest! {
        #[test]
        fn roundtrip_hex_encoded_str(original_script in VariableEncodingNativeScript::any(3)) {
            let original_bytes = to_cbor(&original_script);
            let result = NativeScript::try_from(hex::encode(&original_bytes)).unwrap();

            assert_eq!(result.original_bytes(), &original_bytes);
            assert_eq!(result, NativeScript::from(original_script));
        }
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_script in VariableEncodingNativeScript::any(3)) {
            let original_bytes = to_cbor(&original_script);
            let result: NativeScript = cbor::decode(&original_bytes).unwrap();

            assert_eq!(result.original_bytes(), &original_bytes);
            assert_eq!(result, NativeScript::from(original_script));
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;
    use stacksafe::StackSafe;

    use super::NativeScriptTree;
    use crate::{NativeScript, any_hash28};

    /// A script nested up to `depth` levels, encoded with every array of a definite length.
    pub fn any_native_script(depth: u8) -> impl Strategy<Value = NativeScript> {
        any_native_script_tree(depth).prop_map(NativeScript::new)
    }

    fn any_native_script_tree(depth: u8) -> BoxedStrategy<NativeScriptTree> {
        use NativeScriptTree::*;

        let sig = any_hash28().prop_map(VerificationKey);
        let before = any::<u64>().prop_map(InvalidBefore);
        let after = any::<u64>().prop_map(InvalidAfter);

        if depth > 0 {
            let all = prop::collection::vec(any_native_script_tree(depth - 1), 0..depth as usize)
                .prop_map(StackSafe::new)
                .prop_map(All);
            let some = prop::collection::vec(any_native_script_tree(depth - 1), 0..depth as usize)
                .prop_map(StackSafe::new)
                .prop_map(Any);
            let n_of_k = (any::<i64>(), prop::collection::vec(any_native_script_tree(depth - 1), 0..depth as usize))
                .prop_map(|(n, sigs)| AtLeast(n, StackSafe::new(sigs)));

            prop_oneof![sig, before, after, all, some, n_of_k].boxed()
        } else {
            prop_oneof![sig, before, after].boxed()
        }
    }

    #[cfg(test)]
    mod eval {
        use std::collections::BTreeSet;

        use proptest::prelude::*;
        use test_case::test_case;

        use super::super::NativeScriptTree;
        use crate::{Hash, ValidityInterval, size::KEY};

        #[test_case(vk(1), &[1, 2], always(); "script pubkey present")]
        #[test_case(all(&[vk(1), vk(2)]), &[1, 2], always(); "script all all pass")]
        #[test_case(all(&[]), &[], always(); "script all empty is true")]
        #[test_case(any(&[vk(3), vk(1)]), &[1], always(); "script any one passes")]
        #[test_case(at_least(0, &[vk(9)]), &[1, 2], always(); "script n of k zero always passes")]
        #[test_case(at_least(2, &[vk(1), vk(2), vk(9)]), &[1, 2], always(); "script n of k exact quorum")]
        #[test_case(lock_from(100), &[], after(100); "invalid before with tx start at lock")]
        #[test_case(lock_from(100), &[], after(101); "invalid before with tx start above lock")]
        #[test_case(lock_until(100), &[], before(100); "invalid hereafter with tx expire at lock")]
        #[test_case(lock_until(100), &[], before(50); "invalid hereafter with tx expire below lock")]
        #[test_case(
            all(&[any(&[vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(150, 199);
            "nested all any timelock all conditions pass"
        )]
        fn ok(script: NativeScriptTree, signers: &[u8], validity_interval: ValidityInterval) {
            assert!(script.eval(&verification_key_hashes(signers), validity_interval));
        }

        #[test_case(vk(3), &[1, 2], always(); "script pubkey absent")]
        #[test_case(all(&[vk(1), vk(3)]), &[1, 2], always(); "script all one fails")]
        #[test_case(any(&[vk(3), vk(4)]), &[1, 2], always(); "script any all fail")]
        #[test_case(any(&[]), &[1, 2], always(); "script any empty is false")]
        #[test_case(at_least(2, &[vk(1), vk(8), vk(9)]), &[1, 2], always(); "script n of k just below quorum")]
        #[test_case(at_least(3, &[vk(1), vk(2)]), &[1, 2], always(); "script n of k more than available")]
        #[test_case(lock_from(100), &[], after(99); "invalid before with tx start below lock")]
        #[test_case(lock_from(100), &[], always(); "invalid before without tx start")]
        #[test_case(lock_until(100), &[], before(101); "invalid hereafter with tx expire above lock")]
        #[test_case(lock_until(100), &[], always(); "invalid hereafter without tx expire")]
        #[test_case(
            all(&[any(&[vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(99, 199);
            "nested all any timelock lower bound fails"
        )]
        #[test_case(
            all(&[any(&[vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(150, 201);
            "nested all any timelock upper bound fails"
        )]
        #[test_case(
            all(&[any(&[vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[9],
            between(150, 199);
            "nested all any timelock key check fails"
        )]
        fn ko(script: NativeScriptTree, signers: &[u8], validity_interval: ValidityInterval) {
            assert!(!script.eval(&verification_key_hashes(signers), validity_interval));
        }

        proptest! {
            #[test]
            fn n_of_k_threshold(n in -3i64..6, satisfied in 0usize..4, unsatisfied in 0usize..4) {
                let mut scripts = vec![vk(1); satisfied];
                scripts.extend(vec![vk(9); unsatisfied]);

                assert_eq!(
                    at_least(n, &scripts).eval(&verification_key_hashes(&[1]), always()),
                    satisfied >= n.max(0) as usize,
                );
            }
        }

        fn vk(byte: u8) -> NativeScriptTree {
            NativeScriptTree::verification_key(Hash::from([byte; 28]))
        }

        fn all(scripts: &[NativeScriptTree]) -> NativeScriptTree {
            NativeScriptTree::all(scripts.to_vec())
        }

        fn any(scripts: &[NativeScriptTree]) -> NativeScriptTree {
            NativeScriptTree::any(scripts.to_vec())
        }

        fn at_least(n: i64, scripts: &[NativeScriptTree]) -> NativeScriptTree {
            NativeScriptTree::at_least(n, scripts.to_vec())
        }

        fn lock_from(slot: u64) -> NativeScriptTree {
            NativeScriptTree::invalid_before(slot)
        }

        fn lock_until(slot: u64) -> NativeScriptTree {
            NativeScriptTree::invalid_after(slot)
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

        fn verification_key_hashes(signers: &[u8]) -> BTreeSet<Hash<KEY>> {
            signers.iter().map(|byte| Hash::from([*byte; 28])).collect()
        }
    }

    #[cfg(test)]
    mod iter_filter {
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
    }

    #[cfg(test)]
    mod decoding {
        use test_case::test_case;

        use crate::{NativeScript, cbor, from_cbor, from_cbor_no_leftovers};

        /// A 28-byte key hash of zeroes, as a CBOR byte string. Spelled `K` in the fixtures below.
        const KEY_HASH: &str = "581c00000000000000000000000000000000000000000000000000000000";

        fn decode(template: &str) -> Result<NativeScript, cbor::decode::Error> {
            from_cbor_no_leftovers(&hex::decode(template.replace('K', KEY_HASH)).unwrap())
        }

        #[test_case("8200K"; "script pubkey")]
        #[test_case("820180"; "empty script all, definite")]
        #[test_case("82019fff"; "empty script all, indefinite children")]
        #[test_case("9f0180ff"; "empty script all, indefinite outer")]
        #[test_case("820280"; "empty script any")]
        #[test_case("83030080"; "n of k with no children")]
        #[test_case("820400"; "invalid before")]
        #[test_case("820500"; "invalid hereafter")]
        #[test_case("8201818200K"; "one level of nesting")]
        fn ok(template: &str) {
            assert!(decode(template).is_ok(), "expected {template} to decode");
        }

        #[test_case("820600"; "unknown variant id")]
        #[test_case("8300K00"; "script pubkey with a third element")]
        #[test_case("820300"; "n of k missing its children")]
        #[test_case("8200581c00000000"; "truncated key hash")]
        #[test_case("9f00K"; "indefinite outer array missing its break")]
        #[test_case("82018182 0600"; "nested unknown variant id")]
        #[test_case("820101"; "script all whose children are not an array")]
        #[test_case("82018000"; "trailing bytes after a complete script")]
        fn ko(template: &str) {
            assert!(decode(&template.replace(' ', "")).is_err(), "expected {template} to be rejected");
        }

        #[test]
        fn indefinite_outer_array_arity_is_checked() {
            let bytes = hex::decode("9f00K00ff".replace('K', KEY_HASH)).unwrap();
            assert!(from_cbor::<NativeScript>(&bytes).is_none());
            assert!(from_cbor_no_leftovers::<NativeScript>(&bytes).is_err());
        }
    }

    /// Preprod transaction `f90dce5765108da976abdbb9fc618f9a6ffd9fa4d93b2f288eed1808545424c9`, in
    /// block 5183974, includes a native script holding 5383 levels of nested `ScriptAll`
    /// around a single required signature. A recursive definition of `NativeScript` meant that
    /// this transaction caused a stack overflow.
    ///
    /// Nothing on the network bounds that nesting beyond the maximum transaction size, so
    /// decoding, evaluating, re-encoding, cloning and dropping such a script all have to run
    /// iteratively.
    ///
    /// Each case runs on a thread with an explicitly sized stack. `.cargo/config.toml` raises
    /// `RUST_MIN_STACK` for cargo-launched processes and a deployed node does not inherit it.
    /// A regression here crashes the test binary rather than reporting a failure.
    #[cfg(test)]
    mod depth {
        use std::{collections::BTreeSet, error::Error, thread};

        use crate::{HasScriptHash, Hash, NativeScript, ValidityInterval, from_cbor, size::KEY, to_cbor};

        type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

        const DEFAULT_STACK: usize = 2 * 1024 * 1024;

        const PREPROD_DEPTH: usize = 5383;
        const PREPROD_SCRIPT_HASH: &str = "ff3efca65569f6b0b868a3d34abdb1ad8eccf745e0da71fa94fb4f18";
        const PREPROD_SIGNER: [u8; KEY] = [
            0xba, 0x38, 0x62, 0x09, 0xc0, 0xf8, 0x1f, 0x95, 0x70, 0xb6, 0xfe, 0xb4, 0x5c, 0xed, 0xc2, 0x64, 0x91, 0x44,
            0x44, 0x01, 0x57, 0x67, 0x7c, 0x72, 0x0b, 0xfd, 0x31, 0x4a,
        ];

        /// The deepest a native script can nest and still fit in a transaction: each level of
        /// `ScriptAll` costs three of the 16384 bytes a transaction may occupy.
        const MAX_DEPTH: usize = 16384 / 3;

        #[test]
        fn decodes_the_preprod_transaction_that_broke_indexers() -> TestResult {
            on_a_default_stack(|| {
                let bytes = nested_script(PREPROD_DEPTH, &PREPROD_SIGNER);
                let script: NativeScript = from_cbor(&bytes).ok_or("the script decodes")?;

                assert_eq!(script.script_hash().to_string(), PREPROD_SCRIPT_HASH);
                assert_eq!(script.original_bytes(), bytes);
                assert_eq!(script, script);

                assert!(script.eval(&signers(&[PREPROD_SIGNER]), ValidityInterval::default()));
                assert!(!script.eval(&signers(&[]), ValidityInterval::default()));

                Ok(())
            })
        }

        #[test]
        fn handles_the_deepest_script_a_transaction_can_hold() -> TestResult {
            on_a_default_stack(|| {
                let bytes = nested_script(MAX_DEPTH, &PREPROD_SIGNER);
                let script: NativeScript = from_cbor(&bytes).ok_or("the script decodes")?;

                assert_eq!(to_cbor(&script), bytes);
                assert_eq!(script, script);
                assert!(script.eval(&signers(&[PREPROD_SIGNER]), ValidityInterval::default()));

                let clone = script.clone();
                assert_eq!(clone, script);
                drop(clone);

                Ok(())
            })
        }

        fn nested_script(depth: usize, signer: &[u8; KEY]) -> Vec<u8> {
            const SCRIPT_ALL_OF_ONE: [u8; 3] = [0x82, 0x01, 0x81];
            const SCRIPT_PUBKEY: [u8; 4] = [0x82, 0x00, 0x58, 0x1c];

            let mut bytes = Vec::with_capacity(SCRIPT_ALL_OF_ONE.len() * depth + SCRIPT_PUBKEY.len() + KEY);

            for _ in 0..depth {
                bytes.extend_from_slice(&SCRIPT_ALL_OF_ONE);
            }
            bytes.extend_from_slice(&SCRIPT_PUBKEY);
            bytes.extend_from_slice(signer);

            bytes
        }

        fn signers(keys: &[[u8; KEY]]) -> BTreeSet<Hash<KEY>> {
            keys.iter().map(|key| Hash::from(*key)).collect()
        }

        fn on_a_default_stack(test: impl FnOnce() -> TestResult + Send + 'static) -> TestResult {
            match thread::Builder::new().stack_size(DEFAULT_STACK).spawn(test)?.join() {
                Ok(result) => result,
                Err(_) => Err("the test thread panicked, see the failure reported above".into()),
            }
        }
    }
}
