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

use std::fmt::Debug;

use crate::{
    BootstrapWitness, MemoizedNativeScript, NonEmptySet, NonEmptyVec, PlutusDataSet, PlutusScript, Redeemers,
    VerificationKeyWitness, cbor, protocol_version::PROTOCOL_VERSION_12,
};

/// Transaction witnesses. Key witnesses, scripts, and datums reject duplicate entries from
/// protocol version 12. Vectors preserve the original order and earlier versions' duplicate handling.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    #[cbor(decode_with = "decode_witnesses")]
    pub verification_key_witness: Option<NonEmptyVec<VerificationKeyWitness>>,

    #[n(1)]
    #[cbor(decode_with = "decode_witnesses")]
    pub native_script: Option<NonEmptyVec<MemoizedNativeScript>>,

    #[n(2)]
    #[cbor(decode_with = "decode_witnesses")]
    pub bootstrap_witness: Option<NonEmptyVec<BootstrapWitness>>,

    #[n(3)]
    #[cbor(decode_with = "decode_witnesses")]
    pub plutus_v1_script: Option<NonEmptyVec<PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<PlutusDataSet>,

    #[n(5)]
    pub redeemer: Option<Redeemers>,

    #[n(6)]
    #[cbor(decode_with = "decode_witnesses")]
    pub plutus_v2_script: Option<NonEmptyVec<PlutusScript<2>>>,

    #[n(7)]
    #[cbor(decode_with = "decode_witnesses")]
    pub plutus_v3_script: Option<NonEmptyVec<PlutusScript<3>>>,
}

fn decode_witnesses<'b, C, T>(
    d: &mut cbor::Decoder<'b>,
    ctx: &mut C,
) -> Result<Option<NonEmptyVec<T>>, cbor::decode::Error>
where
    C: cbor::HasProtocolVersion,
    T: Eq + Debug + cbor::Decode<'b, C>,
{
    if ctx.protocol_version() >= PROTOCOL_VERSION_12 {
        d.decode_with::<_, Option<NonEmptySet<T>>>(ctx).map(|set| set.map(NonEmptyVec::from))
    } else {
        d.decode_with(ctx)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::WitnessSet;
    use crate::{
        cbor::from_cbor_no_leftovers_with,
        from_cbor_no_leftovers,
        protocol_version::{PROTOCOL_VERSION_11, PROTOCOL_VERSION_12},
        to_cbor,
    };

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const SIGNATURE: &str = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    fn witness_values() -> [(u8, String); 7] {
        [
            (0, format!("825820{KEY}5840{SIGNATURE}")),
            (1, "820400".to_string()),
            (2, format!("845820{KEY}5840{SIGNATURE}5820{KEY}41a0")),
            (3, "4100".to_string()),
            (4, "00".to_string()),
            (6, "4100".to_string()),
            (7, "4100".to_string()),
        ]
    }

    #[test_case("82", ""; "bare definite")]
    #[test_case("9f", "ff"; "bare indefinite")]
    #[test_case("d9010282", ""; "tagged definite")]
    #[test_case("d901029f", "ff"; "tagged indefinite")]
    fn duplicate_witnesses_are_rejected_from_version_12(prefix: &str, suffix: &str) {
        witness_values().into_iter().for_each(|(field, value)| {
            let bytes = hex::decode(format!("a1{field:02x}{prefix}{value}{value}{suffix}")).unwrap();
            let mut before_version = PROTOCOL_VERSION_11;
            let before = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut before_version);
            assert!(before.is_ok(), "field {field}: {before:?}");
            assert!(from_cbor_no_leftovers::<WitnessSet>(&bytes).is_ok(), "default context, field {field}");

            let mut after_version = PROTOCOL_VERSION_12;
            let after = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut after_version);
            assert!(after.is_err(), "duplicate field {field} accepted at version 12");
            assert!(after.unwrap_err().to_string().contains("duplicate"), "field {field}");
        });
    }

    #[test_case("81", ""; "bare definite")]
    #[test_case("9f", "ff"; "bare indefinite")]
    #[test_case("d9010281", ""; "tagged definite")]
    #[test_case("d901029f", "ff"; "tagged indefinite")]
    fn singleton_witnesses_decode_at_both_versions(prefix: &str, suffix: &str) {
        witness_values().into_iter().for_each(|(field, value)| {
            let bytes = hex::decode(format!("a1{field:02x}{prefix}{value}{suffix}")).unwrap();
            [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12].into_iter().for_each(|mut version| {
                let result = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut version);
                assert!(result.is_ok(), "field {field} at {version:?}: {result:?}");
                let encoded = to_cbor(&result.unwrap());
                let decoded = from_cbor_no_leftovers_with::<_, WitnessSet>(&encoded, &mut version).unwrap();
                assert_eq!(to_cbor(&decoded), encoded);
            });
        });
    }

    #[test_case("80"; "bare definite")]
    #[test_case("9fff"; "bare indefinite")]
    #[test_case("d9010280"; "tagged definite")]
    #[test_case("d901029fff"; "tagged indefinite")]
    fn empty_witness_collections_remain_invalid(collection: &str) {
        witness_values().into_iter().for_each(|(field, _)| {
            let bytes = hex::decode(format!("a1{field:02x}{collection}")).unwrap();
            [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12].into_iter().for_each(|mut version| {
                assert!(
                    from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut version).is_err(),
                    "empty field {field} accepted at {version:?}",
                );
            });
        });
    }

    #[test_case("a101d901028282040082041800"; "native scripts with distinct original bytes")]
    #[test_case("a104d9010282001800"; "datums with distinct original bytes")]
    fn memoized_witnesses_use_original_bytes_for_uniqueness(input: &str) {
        let bytes = hex::decode(input).unwrap();
        let mut version = PROTOCOL_VERSION_12;
        let witnesses = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut version).unwrap();
        assert_eq!(to_cbor(&witnesses), bytes);
    }

    #[test]
    fn an_empty_witness_set_remains_valid() {
        [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12].into_iter().for_each(|mut version| {
            assert_eq!(
                from_cbor_no_leftovers_with::<_, WitnessSet>(&[0xa0], &mut version).unwrap(),
                WitnessSet::default(),
            );
        });
    }

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
}
