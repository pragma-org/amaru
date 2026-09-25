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
    BootstrapWitness, NativeScript, NonEmptyVec, PlutusDataSet, PlutusScript, Redeemers, VerificationKeyWitness, cbor,
};

/// FIXME(cbor): Accidentally not a set
///
///   NonEmptyVec below are supposed to be a NonEmptySet where duplicates would fail to decode. But it isn't.
///   In the Haskell's codebsae, the default decoder for Set fails on duplicate starting from
///   v9 and above:
///
///   <https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Decoding/Decoder.hs#L906-L928>.
///
///   However, the decoders for witnesses fields were (accidentally) overridden and did not use the
///   default `Set` implementation. So, duplicates were silently ignored instead of leading to
///   decoder failure (while still allowing a set tag, and still expecting at least one element):
///
///   <https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L610-L624>
///
///   Importantly, this behaviour is changing again in v12, back to being a non-empty set / maps.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    pub verification_key_witness: Option<NonEmptyVec<VerificationKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptyVec<NativeScript>>,

    /// FIXME(cbor): Accidentally not a set
    ///
    /// See note on verification_key_witness.
    #[n(2)]
    pub bootstrap_witness: Option<NonEmptyVec<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptyVec<PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<PlutusDataSet>,

    #[n(5)]
    pub redeemer: Option<Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptyVec<PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptyVec<PlutusScript<3>>>,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for WitnessSet {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_map_unique_keys(
            d,
            WitnessSet::default(),
            |d| d.u64(),
            |d, st, k| {
                match k {
                    0 => st.verification_key_witness = Some(d.decode_with(ctx)?),
                    1 => st.native_script = Some(d.decode_with(ctx)?),
                    2 => st.bootstrap_witness = Some(d.decode_with(ctx)?),
                    3 => st.plutus_v1_script = Some(d.decode_with(ctx)?),
                    4 => st.plutus_data = Some(d.decode_with(ctx)?),
                    5 => st.redeemer = Some(d.decode_with(ctx)?),
                    6 => st.plutus_v2_script = Some(d.decode_with(ctx)?),
                    7 => st.plutus_v3_script = Some(d.decode_with(ctx)?),
                    _ => {
                        let position = d.position();
                        return Err(cbor::decode::Error::message(format!("unrecognised field key: {k}")).at(position));
                    }
                };

                Ok(())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::WitnessSet;
    use crate::{cbor, from_cbor_no_leftovers, to_cbor};

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const SIGNATURE: &str = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    /// A set of verification key witnesses arrives on-chain in any of three shapes: a bare
    /// definite-length array, an indefinite-length array, or the `#6.258(…)` form the Conway CDDL
    /// prescribes. All three must decode, and all three re-encode to the tagged definite form —
    /// which is why a block carrying either lenient shape never reproduces its own bytes.
    #[test_case("81", ""; "bare definite array, as found on-chain before Conway")]
    #[test_case("9f", "ff"; "indefinite-length array")]
    #[test_case("d9010281", ""; "tagged set, as the Conway CDDL prescribes")]
    fn verification_key_witnesses_always_re_encode_as_a_tagged_definite_set(prefix: &str, suffix: &str) {
        let input = format!("a100{prefix}825820{KEY}5840{SIGNATURE}{suffix}");
        let expected = format!("a100d9010281825820{KEY}5840{SIGNATURE}");

        let witnesses: WitnessSet = from_cbor_no_leftovers(&hex::decode(&input).unwrap()).unwrap();

        let encoded = to_cbor(&witnesses);
        assert_eq!(hex::encode(&encoded), expected, "unexpected encoding");

        let re_decoded: WitnessSet = from_cbor_no_leftovers(&encoded).unwrap();
        assert_eq!(to_cbor(&re_decoded), encoded, "encoding is not a fixed point");
    }

    /// The ledger reads this map with `decodeSparseKeyed`, which fails on a key it does not know
    /// and on a key it has already seen; neither is something to skip or overwrite.
    #[test_case("a0"                              => matches Ok(_)  ; "empty witness set")]
    #[test_case("a10481182a"                      => matches Ok(_)  ; "one known key")]
    #[test_case("a2038141000481182a"              => matches Ok(_)  ; "two distinct known keys")]
    #[test_case("a20481182a0481182a"              => matches Err(_) ; "the same key twice")]
    #[test_case("a1186380"                        => matches Err(_) ; "an unknown key")]
    #[test_case("a20481182a186380"                => matches Err(_) ; "a known key and an unknown one")]
    fn decode_rejects_unknown_and_duplicate_keys(input: &str) -> Result<WitnessSet, cbor::decode::Error> {
        from_cbor_no_leftovers(&hex::decode(input).unwrap())
    }
}
