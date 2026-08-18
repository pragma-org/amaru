// Copyright 2025 PRAGMA
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

use crate::{NativeScript, cbor, utils::string::blanket_try_from_hex_bytes};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "&str")]
pub struct MemoizedNativeScript {
    original_bytes: Vec<u8>,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'expr' should be reflected onto the 'original_bytes'.
    expr: NativeScript,
}

impl MemoizedNativeScript {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl AsRef<NativeScript> for MemoizedNativeScript {
    fn as_ref(&self) -> &NativeScript {
        &self.expr
    }
}

impl TryFrom<&str> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, expr| Self { original_bytes, expr })
    }
}

impl TryFrom<String> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for MemoizedNativeScript {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (expr, original_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;
        Ok(Self { original_bytes: original_bytes.to_vec(), expr })
    }
}

impl<C> cbor::Encode<C> for MemoizedNativeScript {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.original_bytes[..]).map_err(cbor::encode::Error::write)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{Hash, NativeScript, any_hash28, cbor, size::KEY, to_cbor, utils::cbor::CborArray};

    // --------------------------------------------------------------------------------------------
    // Tests
    // --------------------------------------------------------------------------------------------

    proptest! {
        #[test]
        fn roundtrip_hex_encoded_str(original_script in VariableEncodingNativeScript::any(3)) {
            let original_bytes = to_cbor(&original_script);
            let result = MemoizedNativeScript::try_from(hex::encode(&original_bytes)).unwrap();

            assert_eq!(result.as_ref(), &NativeScript::from(original_script));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_script in VariableEncodingNativeScript::any(3)) {
            let original_bytes = to_cbor(&original_script);
            let result: MemoizedNativeScript = cbor::decode(&original_bytes).unwrap();

            assert_eq!(result.as_ref(), &NativeScript::from(original_script));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    // --------------------------------------------------------------------------------------------
    // VariableEncodingNativeScript
    // --------------------------------------------------------------------------------------------

    #[derive(Debug, Clone)]
    enum VariableEncodingNativeScript {
        ScriptPubkey(Hash<KEY>),
        ScriptAll(CborArray<VariableEncodingNativeScript>),
        ScriptAny(CborArray<VariableEncodingNativeScript>),
        ScriptNOfK(i64, CborArray<VariableEncodingNativeScript>),
        InvalidBefore(u64),
        InvalidHereafter(u64),
    }

    impl VariableEncodingNativeScript {
        fn any(depth: u8) -> BoxedStrategy<Self> {
            use VariableEncodingNativeScript::*;

            let sig = any_hash28().prop_map(ScriptPubkey);
            let before = any::<u64>().prop_map(InvalidBefore);
            let after = any::<u64>().prop_map(InvalidHereafter);
            if depth > 0 {
                let all = (any::<bool>(), prop::collection::vec(Self::any(depth - 1), 0..depth as usize)).prop_map(
                    |(is_def, sigs)| ScriptAll(if is_def { CborArray::Def(sigs) } else { CborArray::Indef(sigs) }),
                );

                let some = (any::<bool>(), prop::collection::vec(Self::any(depth - 1), 0..depth as usize)).prop_map(
                    |(is_def, sigs)| ScriptAny(if is_def { CborArray::Def(sigs) } else { CborArray::Indef(sigs) }),
                );

                let n_of_k =
                    (any::<bool>(), any::<i64>(), prop::collection::vec(Self::any(depth - 1), 0..depth as usize))
                        .prop_map(|(is_def, n, sigs)| {
                            ScriptNOfK(n, if is_def { CborArray::Def(sigs) } else { CborArray::Indef(sigs) })
                        });

                prop_oneof![sig, before, after, all, some, n_of_k,].boxed()
            } else {
                prop_oneof![sig, before, after].boxed()
            }
        }
    }

    impl From<VariableEncodingNativeScript> for NativeScript {
        fn from(script: VariableEncodingNativeScript) -> Self {
            use VariableEncodingNativeScript::*;
            match script {
                ScriptPubkey(sig) => Self::ScriptPubkey(sig),
                ScriptAll(sigs) => Self::ScriptAll(Vec::from(sigs).into_iter().map(|s| s.into()).collect()),
                ScriptAny(sigs) => Self::ScriptAny(Vec::from(sigs).into_iter().map(|s| s.into()).collect()),
                ScriptNOfK(n, sigs) => Self::ScriptNOfK(n, Vec::from(sigs).into_iter().map(|s| s.into()).collect()),
                InvalidBefore(n) => Self::InvalidBefore(n),
                InvalidHereafter(n) => Self::InvalidHereafter(n),
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
}
