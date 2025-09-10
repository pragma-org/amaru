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

use crate::{Bytes, KeepRaw, NativeScript, from_cbor, memoized::blanket_try_from_hex_bytes};

use crate::cbor;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "&str")]
pub struct MemoizedNativeScript {
    original_bytes: Bytes,
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

impl TryFrom<Bytes> for MemoizedNativeScript {
    type Error = String;

    fn try_from(original_bytes: Bytes) -> Result<Self, Self::Error> {
        let expr = from_cbor(&original_bytes)
            .ok_or_else(|| "invalid serialized native script".to_string())?;

        Ok(Self {
            original_bytes,
            expr,
        })
    }
}

impl From<KeepRaw<'_, NativeScript>> for MemoizedNativeScript {
    fn from(script: KeepRaw<'_, NativeScript>) -> Self {
        Self {
            original_bytes: Bytes::from(script.raw_cbor().to_vec()),
            expr: script.unwrap(),
        }
    }
}

impl TryFrom<&str> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, expr| Self {
            original_bytes,
            expr,
        })
    }
}

impl TryFrom<String> for MemoizedNativeScript {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<'b, C> cbor::Decode<'b, C> for MemoizedNativeScript {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let start_pos = d.position();
        let expr: NativeScript = d.decode_with(ctx)?;
        let end_pos = d.position();
        let original_bytes = Bytes::from(d.input()[start_pos..end_pos].to_vec());

        Ok(Self {
            original_bytes,
            expr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hash, MaybeIndefArray, cbor, to_cbor};
    use pallas_primitives::conway as pallas;
    use proptest::prelude::*;

    // NOTE: Not using Pallas' type because (a) it has a serialization bug we still need to fix
    // and, (b) it doesn't let us encode native script using unusual encoding choices (e.g. indef
    // vs def arrays).
    #[derive(Debug, Clone)]
    enum NativeScript {
        ScriptPubkey(Hash<28>),
        ScriptAll(MaybeIndefArray<NativeScript>),
        ScriptAny(MaybeIndefArray<NativeScript>),
        ScriptNOfK(u32, MaybeIndefArray<NativeScript>),
        InvalidBefore(u64),
        InvalidHereafter(u64),
    }

    impl From<NativeScript> for pallas::NativeScript {
        fn from(script: NativeScript) -> Self {
            match script {
                NativeScript::ScriptPubkey(sig) => Self::ScriptPubkey(sig),
                NativeScript::ScriptAll(sigs) => {
                    Self::ScriptAll(sigs.to_vec().into_iter().map(|s| s.into()).collect())
                }
                NativeScript::ScriptAny(sigs) => {
                    Self::ScriptAny(sigs.to_vec().into_iter().map(|s| s.into()).collect())
                }
                NativeScript::ScriptNOfK(n, sigs) => {
                    Self::ScriptNOfK(n, sigs.to_vec().into_iter().map(|s| s.into()).collect())
                }
                NativeScript::InvalidBefore(n) => Self::InvalidBefore(n),
                NativeScript::InvalidHereafter(n) => Self::InvalidHereafter(n),
            }
        }
    }

    impl<C> cbor::encode::Encode<C> for NativeScript {
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

    prop_compose! {
        pub(crate) fn any_key_hash()(bytes in any::<[u8; 28]>()) -> Hash<28> {
            Hash::from(bytes)
        }
    }

    fn any_native_script(depth: u8) -> BoxedStrategy<NativeScript> {
        let sig = any_key_hash().prop_map(NativeScript::ScriptPubkey);
        let before = any::<u64>().prop_map(NativeScript::InvalidBefore);
        let after = any::<u64>().prop_map(NativeScript::InvalidHereafter);
        if depth > 0 {
            let all = (
                any::<bool>(),
                prop::collection::vec(any_native_script(depth - 1), 0..depth as usize),
            )
                .prop_map(|(is_def, sigs)| {
                    NativeScript::ScriptAll(if is_def {
                        MaybeIndefArray::Def(sigs)
                    } else {
                        MaybeIndefArray::Indef(sigs)
                    })
                });

            let some = (
                any::<bool>(),
                prop::collection::vec(any_native_script(depth - 1), 0..depth as usize),
            )
                .prop_map(|(is_def, sigs)| {
                    NativeScript::ScriptAny(if is_def {
                        MaybeIndefArray::Def(sigs)
                    } else {
                        MaybeIndefArray::Indef(sigs)
                    })
                });

            let n_of_k = (
                any::<bool>(),
                any::<u32>(),
                prop::collection::vec(any_native_script(depth - 1), 0..depth as usize),
            )
                .prop_map(|(is_def, n, sigs)| {
                    NativeScript::ScriptNOfK(
                        n,
                        if is_def {
                            MaybeIndefArray::Def(sigs)
                        } else {
                            MaybeIndefArray::Indef(sigs)
                        },
                    )
                });

            prop_oneof![sig, before, after, all, some, n_of_k,].boxed()
        } else {
            prop_oneof![sig, before, after].boxed()
        }
    }

    proptest! {
        #[test]
        fn roundtrip_hex_encoded_str(original_script in any_native_script(3)) {
            let original_bytes = to_cbor(&original_script);
            let result = MemoizedNativeScript::try_from(hex::encode(&original_bytes)).unwrap();

            assert_eq!(result.as_ref(), &pallas::NativeScript::from(original_script));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_script in any_native_script(3)) {
            let original_bytes = to_cbor(&original_script);
            let raw: KeepRaw<'_, pallas::NativeScript> = cbor::decode(&original_bytes).unwrap();
            let result: MemoizedNativeScript = raw.into();

            assert_eq!(result.as_ref(), &pallas::NativeScript::from(original_script));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }
}
