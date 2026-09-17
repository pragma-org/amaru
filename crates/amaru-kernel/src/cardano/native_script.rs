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

use crate::{Hash, ValidityInterval, cbor, from_cbor, size::KEY, to_cbor};

/// A native script, held as the bytes it was decoded from together with its nodes in pre-order,
/// which is the order those bytes lay them out in.
///
/// A script hash is taken over the bytes, and definite and indefinite encodings of the same tree
/// hash differently, so a script keeps the bytes it arrived with instead of re-deriving them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "&str")]
pub struct NativeScript {
    original_bytes: Vec<u8>,
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    ScriptPubkey(Hash<{ KEY }>),
    InvalidBefore(u64),
    InvalidHereafter(u64),
    ScriptAll { children: usize },
    ScriptAny { children: usize },
    ScriptNOfK { n: i64, children: usize },
}

impl NativeScript {
    pub fn script_pubkey(hash: Hash<{ KEY }>) -> Self {
        Self::new(vec![Node::ScriptPubkey(hash)])
    }

    pub fn invalid_before(slot: u64) -> Self {
        Self::new(vec![Node::InvalidBefore(slot)])
    }

    pub fn invalid_hereafter(slot: u64) -> Self {
        Self::new(vec![Node::InvalidHereafter(slot)])
    }

    pub fn all(scripts: Vec<Self>) -> Self {
        Self::branch(scripts, |children| Node::ScriptAll { children })
    }

    pub fn any(scripts: Vec<Self>) -> Self {
        Self::branch(scripts, |children| Node::ScriptAny { children })
    }

    pub fn at_least(n: i64, scripts: Vec<Self>) -> Self {
        Self::branch(scripts, move |children| Node::ScriptNOfK { n, children })
    }

    fn branch(scripts: Vec<Self>, branch: impl FnOnce(usize) -> Node) -> Self {
        let mut nodes = vec![branch(scripts.len())];
        nodes.extend(scripts.into_iter().flat_map(|script| script.nodes));

        Self::new(nodes)
    }

    fn new(nodes: Vec<Node>) -> Self {
        Self { original_bytes: to_cbor(&Nodes(&nodes)), nodes }
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn eval(&self, verification_key_hashes: &BTreeSet<Hash<KEY>>, validity_interval: ValidityInterval) -> bool {
        let mut results: Vec<bool> = Vec::with_capacity(self.nodes.len());
        for node in self.nodes.iter().rev() {
            let satisfied = match *node {
                Node::ScriptPubkey(key) => verification_key_hashes.contains(&key),
                Node::ScriptAll { children } => take(&mut results, children).all(|satisfied| satisfied),
                Node::ScriptAny { children } => take(&mut results, children).any(|satisfied| satisfied),
                Node::ScriptNOfK { n, children } => {
                    let n = n.max(0) as usize;
                    take(&mut results, children).filter(|satisfied| *satisfied).take(n).count() == n
                }
                // `lteNegInfty`: a lock requiring `lock_start <= ValidityInterval::lower_bound()` can only be satisfied when
                // `tx_start` is given. A missing lower bound is treated as -inf and always fails.
                Node::InvalidBefore(lock_start) => {
                    validity_interval.lower_bound().is_some_and(|t| lock_start <= t.as_u64())
                }
                // `ltePosInfty`: a lock requiring `ValidityInterval::upper_bound() <= lock_expire` can only be satisfied when
                // `tx_expire` is given. A missing upper bound is treated as +inf and always fails.
                Node::InvalidHereafter(lock_expire) => {
                    validity_interval.upper_bound().is_some_and(|t| t.as_u64() <= lock_expire)
                }
            };

            results.push(satisfied);
        }

        results.pop().unwrap_or_else(|| unreachable!("a script has at least one node"))
    }
}

/// Detach the results of a branch's children from the top of the stack.
fn take(results: &mut Vec<bool>, children: usize) -> impl Iterator<Item = bool> + '_ {
    let at = results
        .len()
        .checked_sub(children)
        .unwrap_or_else(|| unreachable!("a branch's children are evaluated before it"));

    results.drain(at..)
}

impl serde::Serialize for NativeScript {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.original_bytes))
    }
}

impl TryFrom<&str> for NativeScript {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let original_bytes = hex::decode(s).map_err(|e| e.to_string())?;

        from_cbor(&original_bytes).ok_or_else(|| "failed to decode from CBOR".to_string())
    }
}

impl TryFrom<String> for NativeScript {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl Node {
    /// Fill in the child count of a branch whose children array ended on a break.
    fn set_children(&mut self, count: usize) {
        match self {
            Self::ScriptAll { children } | Self::ScriptAny { children } | Self::ScriptNOfK { children, .. } => {
                *children = count
            }
            Self::ScriptPubkey(..) | Self::InvalidBefore(..) | Self::InvalidHereafter(..) => {
                unreachable!("only a branch node opens a frame")
            }
        }
    }
}

/// A branch whose children are still being decoded. `outer` is the length of the array holding the
/// variant id, or `None` when that array is indefinite and so owes a break.
enum Frame {
    /// A children array of a known length, which is the child count outright.
    Definite { remaining: u64, outer: Option<u64> },
    /// A children array running until a break, so the branch's node, at `index`, is still waiting
    /// for its count.
    Indefinite { index: usize, counted: usize, outer: Option<u64> },
}

impl Frame {
    /// Claim the next child slot, reporting whether one is there. Consumes the break of an
    /// indefinite children array once it is reached.
    fn next_child(&mut self, d: &mut cbor::Decoder<'_>) -> Result<bool, cbor::decode::Error> {
        match self {
            Self::Definite { remaining: 0, .. } => Ok(false),
            Self::Definite { remaining, .. } => {
                *remaining -= 1;
                Ok(true)
            }
            Self::Indefinite { counted, .. } => {
                if cbor::decode_break(d, None)? {
                    return Ok(false);
                }
                *counted += 1;
                Ok(true)
            }
        }
    }

    fn outer(&self) -> Option<u64> {
        match *self {
            Self::Definite { outer, .. } | Self::Indefinite { outer, .. } => outer,
        }
    }
}

/// Start a branch's children array, reporting the child count to record on its node. A definite
/// length is that count outright; an indefinite one is counted as it goes and filled in on close.
fn open(stack: &mut Vec<Frame>, len: Option<u64>, index: usize, outer: Option<u64>) -> usize {
    match len {
        Some(remaining) => {
            stack.push(Frame::Definite { remaining, outer });
            remaining as usize
        }
        None => {
            stack.push(Frame::Indefinite { index, counted: 0, outer });
            0
        }
    }
}

fn assert_len(len: Option<u64>, expected: u64) -> Result<(), cbor::decode::Error> {
    match len {
        Some(len) if len != expected => {
            Err(cbor::decode::Error::message(format!("CBOR array length mismatch: expected {expected} got {len}")))
        }
        _ => Ok(()),
    }
}

/// Consume the break closing an indefinite array. A definite one ends on its own.
fn close(d: &mut cbor::Decoder<'_>, len: Option<u64>) -> Result<(), cbor::decode::Error> {
    if len.is_none() {
        cbor::decode_break(d, len)?;
    }

    Ok(())
}

fn decode_nodes<C: cbor::HasProtocolVersion>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<Vec<Node>, cbor::decode::Error> {
    let mut nodes = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    loop {
        let outer = d.array()?;
        let index = nodes.len();

        match d.u32()? {
            0 => {
                assert_len(outer, 2)?;
                nodes.push(Node::ScriptPubkey(d.decode_with(ctx)?));
                close(d, outer)?;
            }
            1 => {
                assert_len(outer, 2)?;
                nodes.push(Node::ScriptAll { children: open(&mut stack, d.array()?, index, outer) });
            }
            2 => {
                assert_len(outer, 2)?;
                nodes.push(Node::ScriptAny { children: open(&mut stack, d.array()?, index, outer) });
            }
            3 => {
                assert_len(outer, 3)?;
                let n = d.decode_with(ctx)?;
                nodes.push(Node::ScriptNOfK { n, children: open(&mut stack, d.array()?, index, outer) });
            }
            4 => {
                assert_len(outer, 2)?;
                nodes.push(Node::InvalidBefore(d.decode_with(ctx)?));
                close(d, outer)?;
            }
            5 => {
                assert_len(outer, 2)?;
                nodes.push(Node::InvalidHereafter(d.decode_with(ctx)?));
                close(d, outer)?;
            }
            _ => return Err(cbor::decode::Error::message("unknown variant id for native script")),
        }

        while let Some(frame) = stack.last_mut() {
            if frame.next_child(d)? {
                break;
            }

            let frame = stack.pop().unwrap_or_else(|| unreachable!("frame observed on the line above"));
            close(d, frame.outer())?;

            if let Frame::Indefinite { index, counted, .. } = frame {
                nodes[index].set_children(counted);
            }
        }

        if stack.is_empty() {
            return Ok(nodes);
        }
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for NativeScript {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (nodes, original_bytes) = cbor::tee(d, |d| decode_nodes(d, ctx))?;

        Ok(Self { original_bytes: original_bytes.to_vec(), nodes })
    }
}

impl<C> cbor::encode::Encode<C> for NativeScript {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.original_bytes).map_err(cbor::encode::Error::write)
    }
}

/// A node sequence encoded with every array of a definite length, which is what a script assembled
/// through [`NativeScript`]'s constructors is given for its original bytes.
struct Nodes<'a>(&'a [Node]);

impl<C> cbor::encode::Encode<C> for Nodes<'_> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        for node in self.0 {
            match *node {
                Node::ScriptPubkey(hash) => {
                    e.array(2)?;
                    e.encode_with(0, ctx)?;
                    e.encode_with(hash, ctx)?;
                }
                Node::ScriptAll { children } => {
                    e.array(2)?;
                    e.encode_with(1, ctx)?;
                    e.array(children as u64)?;
                }
                Node::ScriptAny { children } => {
                    e.array(2)?;
                    e.encode_with(2, ctx)?;
                    e.array(children as u64)?;
                }
                Node::ScriptNOfK { n, children } => {
                    e.array(3)?;
                    e.encode_with(3, ctx)?;
                    e.encode_with(n, ctx)?;
                    e.array(children as u64)?;
                }
                Node::InvalidBefore(slot) => {
                    e.array(2)?;
                    e.encode_with(4, ctx)?;
                    e.encode_with(slot, ctx)?;
                }
                Node::InvalidHereafter(slot) => {
                    e.array(2)?;
                    e.encode_with(5, ctx)?;
                    e.encode_with(slot, ctx)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::NativeScript;
    use crate::{Hash, any_hash28, cbor, size::KEY, utils::cbor::CborArray};

    /// A script nested up to `depth` levels, encoded with every array of a definite length.
    pub fn any_native_script(depth: u8) -> BoxedStrategy<NativeScript> {
        VariableEncodingNativeScript::any(depth).prop_map(NativeScript::from).boxed()
    }

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
        pub fn any(depth: u8) -> BoxedStrategy<Self> {
            use VariableEncodingNativeScript::*;

            let sig = any_hash28().prop_map(ScriptPubkey);
            let before = any::<u64>().prop_map(InvalidBefore);
            let after = any::<u64>().prop_map(InvalidHereafter);

            if depth > 0 {
                let all = children(depth).prop_map(ScriptAll);
                let some = children(depth).prop_map(ScriptAny);
                let n_of_k = (any::<i64>(), children(depth)).prop_map(|(n, scripts)| ScriptNOfK(n, scripts));

                prop_oneof![sig, before, after, all, some, n_of_k,].boxed()
            } else {
                prop_oneof![sig, before, after].boxed()
            }
        }
    }

    fn children(depth: u8) -> impl Strategy<Value = CborArray<VariableEncodingNativeScript>> {
        (any::<bool>(), prop::collection::vec(VariableEncodingNativeScript::any(depth - 1), 0..MAX_BREADTH)).prop_map(
            |(is_definite, scripts)| {
                if is_definite { CborArray::Def(scripts) } else { CborArray::Indef(scripts) }
            },
        )
    }

    const MAX_BREADTH: usize = 3;

    impl From<VariableEncodingNativeScript> for NativeScript {
        fn from(script: VariableEncodingNativeScript) -> Self {
            use VariableEncodingNativeScript::*;

            fn convert(scripts: CborArray<VariableEncodingNativeScript>) -> Vec<NativeScript> {
                Vec::from(scripts).into_iter().map(NativeScript::from).collect()
            }

            match script {
                ScriptPubkey(sig) => Self::script_pubkey(sig),
                ScriptAll(sigs) => Self::all(convert(sigs)),
                ScriptAny(sigs) => Self::any(convert(sigs)),
                ScriptNOfK(n, sigs) => Self::at_least(n, convert(sigs)),
                InvalidBefore(n) => Self::invalid_before(n),
                InvalidHereafter(n) => Self::invalid_hereafter(n),
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

    #[cfg(test)]
    mod internal {
        use std::collections::BTreeSet;

        use proptest::prelude::*;
        use test_case::test_case;

        use crate::{Hash, NativeScript, ValidityInterval, size::KEY};

        #[test_case(vk(1), &[1, 2], always(); "script pubkey present")]
        #[test_case(all([vk(1), vk(2)]), &[1, 2], always(); "script all all pass")]
        #[test_case(all([]), &[], always(); "script all empty is true")]
        #[test_case(any([vk(3), vk(1)]), &[1], always(); "script any one passes")]
        #[test_case(at_least(0, [vk(9)]), &[1, 2], always(); "script n of k zero always passes")]
        #[test_case(at_least(2, [vk(1), vk(2), vk(9)]), &[1, 2], always(); "script n of k exact quorum")]
        #[test_case(lock_from(100), &[], after(100); "invalid before with tx start at lock")]
        #[test_case(lock_from(100), &[], after(101); "invalid before with tx start above lock")]
        #[test_case(lock_until(100), &[], before(100); "invalid hereafter with tx expire at lock")]
        #[test_case(lock_until(100), &[], before(50); "invalid hereafter with tx expire below lock")]
        #[test_case(
            all([any([vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(150, 199);
            "nested all any timelock all conditions pass"
        )]
        fn ok(script: NativeScript, signers: &[u8], validity_interval: ValidityInterval) {
            assert!(script.eval(&verification_key_hashes(signers), validity_interval));
        }

        #[test_case(vk(3), &[1, 2], always(); "script pubkey absent")]
        #[test_case(all([vk(1), vk(3)]), &[1, 2], always(); "script all one fails")]
        #[test_case(any([vk(3), vk(4)]), &[1, 2], always(); "script any all fail")]
        #[test_case(any([]), &[1, 2], always(); "script any empty is false")]
        #[test_case(at_least(2, [vk(1), vk(8), vk(9)]), &[1, 2], always(); "script n of k just below quorum")]
        #[test_case(at_least(3, [vk(1), vk(2)]), &[1, 2], always(); "script n of k more than available")]
        #[test_case(lock_from(100), &[], after(99); "invalid before with tx start below lock")]
        #[test_case(lock_from(100), &[], always(); "invalid before without tx start")]
        #[test_case(lock_until(100), &[], before(101); "invalid hereafter with tx expire above lock")]
        #[test_case(lock_until(100), &[], always(); "invalid hereafter without tx expire")]
        #[test_case(
            all([any([vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(99, 199);
            "nested all any timelock lower bound fails"
        )]
        #[test_case(
            all([any([vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[1],
            between(150, 201);
            "nested all any timelock upper bound fails"
        )]
        #[test_case(
            all([any([vk(8), vk(1)]), lock_from(100), lock_until(200)]),
            &[9],
            between(150, 199);
            "nested all any timelock key check fails"
        )]
        fn ko(script: NativeScript, signers: &[u8], validity_interval: ValidityInterval) {
            assert!(!script.eval(&verification_key_hashes(signers), validity_interval));
        }

        proptest! {
            #[test]
            fn n_of_k_threshold(n in -3i64..6, satisfied in 0usize..4, unsatisfied in 0usize..4) {
                let mut scripts = vec![vk(1); satisfied];
                scripts.extend(vec![vk(9); unsatisfied]);

                assert_eq!(
                    NativeScript::at_least(n, scripts).eval(&verification_key_hashes(&[1]), always()),
                    satisfied >= n.max(0) as usize,
                );
            }
        }

        fn vk(byte: u8) -> NativeScript {
            NativeScript::script_pubkey(Hash::from([byte; 28]))
        }

        fn all<const N: usize>(scripts: [NativeScript; N]) -> NativeScript {
            NativeScript::all(scripts.into())
        }

        fn any<const N: usize>(scripts: [NativeScript; N]) -> NativeScript {
            NativeScript::any(scripts.into())
        }

        fn at_least<const N: usize>(n: i64, scripts: [NativeScript; N]) -> NativeScript {
            NativeScript::at_least(n, scripts.into())
        }

        fn lock_from(slot: u64) -> NativeScript {
            NativeScript::invalid_before(slot)
        }

        fn lock_until(slot: u64) -> NativeScript {
            NativeScript::invalid_hereafter(slot)
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
    mod differential {
        use std::collections::BTreeSet;

        use proptest::prelude::*;

        use super::VariableEncodingNativeScript;
        use crate::{Hash, NativeScript, Slot, ValidityInterval, from_cbor_no_leftovers, size::KEY, to_cbor};

        proptest! {
            #[test]
            fn eval_agrees_with_the_recursive_definition(
                script in VariableEncodingNativeScript::any(6),
                signed in any::<u64>(),
                lower_bound in any::<Option<u64>>(),
                upper_bound in any::<Option<u64>>(),
            ) {
                let validity_interval = ValidityInterval::new(lower_bound.map(Slot::from), upper_bound.map(Slot::from));
                let verification_key_hashes = signers(&script, signed);
                let flat: NativeScript = from_cbor_no_leftovers(&to_cbor(&script)).unwrap();

                assert_eq!(
                    flat.eval(&verification_key_hashes, validity_interval),
                    eval(&script, &verification_key_hashes, validity_interval),
                );
            }
        }

        /// The recursive definition of native script evaluation, as an oracle for the iterative one.
        fn eval(
            script: &VariableEncodingNativeScript,
            verification_key_hashes: &BTreeSet<Hash<KEY>>,
            validity_interval: ValidityInterval,
        ) -> bool {
            use VariableEncodingNativeScript::*;

            match script {
                ScriptPubkey(key) => verification_key_hashes.contains(key),
                ScriptAll(scripts) => scripts.iter().all(|s| eval(s, verification_key_hashes, validity_interval)),
                ScriptAny(scripts) => scripts.iter().any(|s| eval(s, verification_key_hashes, validity_interval)),
                ScriptNOfK(n, scripts) => {
                    let n = (*n).max(0) as usize;
                    scripts.iter().filter(|s| eval(s, verification_key_hashes, validity_interval)).take(n).count() == n
                }
                InvalidBefore(lock_start) => validity_interval.lower_bound().is_some_and(|t| lock_start <= &t.as_u64()),
                InvalidHereafter(lock_expire) => {
                    validity_interval.upper_bound().is_some_and(|t| &t.as_u64() <= lock_expire)
                }
            }
        }

        /// Sign with the subset of the script's own keys that `mask` picks out, so that signature
        /// checks can go either way rather than always failing on unrelated hashes.
        fn signers(script: &VariableEncodingNativeScript, mask: u64) -> BTreeSet<Hash<KEY>> {
            let mut keys = Vec::new();
            collect_keys(script, &mut keys);

            keys.into_iter()
                .enumerate()
                .filter(|(index, _)| ((mask >> (index % 64)) & 1) == 1)
                .map(|(_, key)| key)
                .collect()
        }

        fn collect_keys(script: &VariableEncodingNativeScript, keys: &mut Vec<Hash<KEY>>) {
            use VariableEncodingNativeScript::*;

            match script {
                ScriptPubkey(key) => keys.push(*key),
                ScriptAll(scripts) | ScriptAny(scripts) | ScriptNOfK(_, scripts) => {
                    scripts.iter().for_each(|script| collect_keys(script, keys))
                }
                InvalidBefore(..) | InvalidHereafter(..) => (),
            }
        }
    }

    #[cfg(test)]
    mod decoding {
        use proptest::prelude::*;
        use test_case::test_case;

        use super::VariableEncodingNativeScript;
        use crate::{NativeScript, cbor, from_cbor, from_cbor_no_leftovers, to_cbor};

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

        /// `heterogeneous_array` only enforces the element count for definite-length arrays, so an
        /// indefinite outer array carrying extra elements decodes, stopping short of its own break
        /// and leaving the remainder in the stream.
        #[test]
        fn indefinite_outer_array_arity_is_unchecked() {
            let bytes = hex::decode("9f00K00ff".replace('K', KEY_HASH)).unwrap();

            assert!(from_cbor::<NativeScript>(&bytes).is_some());
            assert!(from_cbor_no_leftovers::<NativeScript>(&bytes).is_err());
        }

        proptest! {
            #[test]
            fn decoding_yields_the_tree_and_keeps_the_bytes(original in VariableEncodingNativeScript::any(8)) {
                let bytes = to_cbor(&original);
                let decoded: NativeScript = from_cbor_no_leftovers(&bytes).unwrap();

                assert_eq!(decoded.nodes, NativeScript::from(original).nodes);
                assert_eq!(decoded.original_bytes(), bytes);
                assert_eq!(to_cbor(&decoded), bytes);
            }
        }

        proptest! {
            #[test]
            fn decoding_from_hex_matches_decoding_from_bytes(original in VariableEncodingNativeScript::any(3)) {
                let bytes = to_cbor(&original);

                assert_eq!(
                    NativeScript::try_from(hex::encode(&bytes)).unwrap(),
                    from_cbor_no_leftovers::<NativeScript>(&bytes).unwrap(),
                );
            }
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
