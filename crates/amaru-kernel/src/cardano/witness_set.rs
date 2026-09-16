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
    BootstrapWitness, Duplicates, MemoizedNativeScript, NonEmptySet, PlutusDataSet, PlutusScript, Redeemers,
    VerificationKeyWitness, cbor, protocol_version::PROTOCOL_VERSION_12,
};

/// Transaction witnesses.
///
/// Every collection is a [`NonEmptySet`]: the type rules out empty collections and duplicate
/// entries. What happens to a duplicate entry in the CBOR input depends on the field and on the
/// protocol version, as in the Haskell ledger:
///
/// - Verification key witnesses, native scripts, bootstrap witnesses and datums: duplicates are
///   rejected from protocol version 12. Before that, the last of two same entries is kept, as
///   `Set.fromList` and `Map.fromList` do in the
///   [witness decoders](https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L610-L679)
///   and the [datum decoder](https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L329-L348).
/// - Plutus scripts: duplicates are rejected at every protocol version this node supports, see
///   [`scriptDecoderV9`](https://github.com/IntersectMBO/cardano-ledger/blob/fe0af09c8667bf8ffdd17dd1a387515b9b0533bf/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L700-L722).
///
/// Two bootstrap witnesses are the same when they share their key hash, see
/// [`BootstrapWitness::has_same_key_hash`]. Other entries compare by value; memoized entries
/// compare by their original bytes, which is what the ledger hashes. The set tag 258 stays
/// optional on input and is always written on output.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    #[cbor(decode_with = "decode_verification_key_witnesses")]
    pub verification_key_witness: Option<NonEmptySet<VerificationKeyWitness>>,

    #[n(1)]
    #[cbor(decode_with = "decode_native_scripts")]
    pub native_script: Option<NonEmptySet<MemoizedNativeScript>>,

    #[n(2)]
    #[cbor(decode_with = "decode_bootstrap_witnesses")]
    pub bootstrap_witness: Option<NonEmptySet<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptySet<PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<PlutusDataSet>,

    #[n(5)]
    pub redeemer: Option<Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptySet<PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptySet<PlutusScript<3>>>,
}

/// Duplicate policy for witness collections at `ctx`'s protocol version: rejected from version 12,
/// last entry kept before it, as the Haskell ledger does.
pub(crate) fn duplicate_witness_policy<C: cbor::HasProtocolVersion>(ctx: &C) -> Duplicates {
    if ctx.protocol_version() >= PROTOCOL_VERSION_12 { Duplicates::Reject } else { Duplicates::KeepLast }
}

fn decode_witnesses<'b, C, T>(
    d: &mut cbor::Decoder<'b>,
    ctx: &mut C,
    same: impl Fn(&T, &T) -> bool,
) -> Result<Option<NonEmptySet<T>>, cbor::decode::Error>
where
    C: cbor::HasProtocolVersion,
    T: Eq + cbor::Decode<'b, C>,
{
    let duplicates = duplicate_witness_policy(ctx);

    // Mirror minicbor's `Option<T>` decoder: a CBOR null is `None`.
    if d.datatype()? == cbor::Type::Null {
        d.null().map(|()| None)
    } else {
        NonEmptySet::decode_by(d, ctx, same, duplicates).map(Some)
    }
}

fn decode_verification_key_witnesses<'b, C: cbor::HasProtocolVersion>(
    d: &mut cbor::Decoder<'b>,
    ctx: &mut C,
) -> Result<Option<NonEmptySet<VerificationKeyWitness>>, cbor::decode::Error> {
    decode_witnesses(d, ctx, VerificationKeyWitness::eq)
}

fn decode_native_scripts<'b, C: cbor::HasProtocolVersion>(
    d: &mut cbor::Decoder<'b>,
    ctx: &mut C,
) -> Result<Option<NonEmptySet<MemoizedNativeScript>>, cbor::decode::Error> {
    decode_witnesses(d, ctx, MemoizedNativeScript::eq)
}

fn decode_bootstrap_witnesses<'b, C: cbor::HasProtocolVersion>(
    d: &mut cbor::Decoder<'b>,
    ctx: &mut C,
) -> Result<Option<NonEmptySet<BootstrapWitness>>, cbor::decode::Error> {
    decode_witnesses(d, ctx, BootstrapWitness::has_same_key_hash)
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
    const OTHER_SIGNATURE: &str = "01010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101";

    /// Witness fields whose duplicates the ledger deduplicates before protocol version 12.
    fn deduplicated_witness_values() -> [(u8, String); 4] {
        [
            (0, format!("825820{KEY}5840{SIGNATURE}")),
            (1, "820400".to_string()),
            (2, format!("845820{KEY}5840{SIGNATURE}5820{KEY}41a0")),
            (4, "00".to_string()),
        ]
    }

    /// Plutus script fields, whose duplicates the ledger rejects at every version.
    fn plutus_script_values() -> [(u8, String); 3] {
        [(3, "4100".to_string()), (6, "4100".to_string()), (7, "4100".to_string())]
    }

    /// Number of entries in the collection of `field`, or `None` when the field is absent.
    fn collection_len(witnesses: &WitnessSet, field: u8) -> Option<usize> {
        match field {
            0 => witnesses.verification_key_witness.as_ref().map(|xs| xs.len()),
            1 => witnesses.native_script.as_ref().map(|xs| xs.len()),
            2 => witnesses.bootstrap_witness.as_ref().map(|xs| xs.len()),
            4 => witnesses.plutus_data.as_ref().map(|xs| xs.len()),
            _ => None,
        }
    }

    #[test_case("82", ""; "bare definite")]
    #[test_case("9f", "ff"; "bare indefinite")]
    #[test_case("d9010282", ""; "tagged definite")]
    #[test_case("d901029f", "ff"; "tagged indefinite")]
    fn duplicate_witnesses_are_deduplicated_before_version_12_and_rejected_from_it(prefix: &str, suffix: &str) {
        deduplicated_witness_values().into_iter().for_each(|(field, value)| {
            let bytes = hex::decode(format!("a1{field:02x}{prefix}{value}{value}{suffix}")).unwrap();

            let mut before_version = PROTOCOL_VERSION_11;
            let before = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut before_version);
            assert_eq!(
                before.as_ref().ok().and_then(|witnesses| collection_len(witnesses, field)),
                Some(1),
                "field {field} at version 11: {before:?}",
            );

            let by_default = from_cbor_no_leftovers::<WitnessSet>(&bytes);
            assert_eq!(
                by_default.as_ref().ok().and_then(|witnesses| collection_len(witnesses, field)),
                Some(1),
                "field {field} in the default context: {by_default:?}",
            );

            let mut after_version = PROTOCOL_VERSION_12;
            let after = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut after_version);
            assert!(after.is_err(), "duplicate field {field} accepted at version 12");
            assert!(after.unwrap_err().to_string().contains("duplicate"), "field {field}");
        });
    }

    #[test_case("82", ""; "bare definite")]
    #[test_case("9f", "ff"; "bare indefinite")]
    #[test_case("d9010282", ""; "tagged definite")]
    #[test_case("d901029f", "ff"; "tagged indefinite")]
    fn duplicate_plutus_scripts_are_rejected_at_every_version(prefix: &str, suffix: &str) {
        plutus_script_values().into_iter().for_each(|(field, value)| {
            let bytes = hex::decode(format!("a1{field:02x}{prefix}{value}{value}{suffix}")).unwrap();

            [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12].into_iter().for_each(|mut version| {
                let result = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut version);
                assert!(result.is_err(), "duplicate field {field} accepted at {version:?}");
                assert!(result.unwrap_err().to_string().contains("duplicate"), "field {field} at {version:?}");
            });

            let by_default = from_cbor_no_leftovers::<WitnessSet>(&bytes);
            assert!(by_default.is_err(), "duplicate field {field} accepted in the default context");
            assert!(by_default.unwrap_err().to_string().contains("duplicate"), "field {field} by default");
        });
    }

    /// Two bootstrap witnesses that share their key, chain code and attributes are the same witness
    /// even when their signatures differ, because the ledger keys them on `bootstrapWitKeyHash`.
    #[test]
    fn bootstrap_witnesses_with_the_same_key_hash_are_duplicates() {
        let first = format!("845820{KEY}5840{SIGNATURE}5820{KEY}41a0");
        let second = format!("845820{KEY}5840{OTHER_SIGNATURE}5820{KEY}41a0");
        let bytes = hex::decode(format!("a102d9010282{first}{second}")).unwrap();

        let mut before_version = PROTOCOL_VERSION_11;
        let before = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut before_version).unwrap();
        assert_eq!(collection_len(&before, 2), Some(1), "the two witnesses did not collapse into one");
        assert_eq!(
            hex::encode(to_cbor(&before)),
            format!("a102d9010281{second}"),
            "the surviving witness is not the last one",
        );

        let mut after_version = PROTOCOL_VERSION_12;
        let after = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut after_version);
        assert!(after.is_err(), "duplicate bootstrap witnesses accepted at version 12");
        assert!(after.unwrap_err().to_string().contains("duplicate"));
    }

    /// Verification key witnesses, unlike bootstrap witnesses, compare on their signature too.
    #[test]
    fn verification_key_witnesses_with_the_same_key_but_another_signature_are_distinct() {
        let first = format!("825820{KEY}5840{SIGNATURE}");
        let second = format!("825820{KEY}5840{OTHER_SIGNATURE}");
        let bytes = hex::decode(format!("a100d9010282{first}{second}")).unwrap();

        [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12].into_iter().for_each(|mut version| {
            let result = from_cbor_no_leftovers_with::<_, WitnessSet>(&bytes, &mut version);
            assert!(result.is_ok(), "at {version:?}: {result:?}");
            assert_eq!(collection_len(&result.unwrap(), 0), Some(2), "at {version:?}");
        });
    }

    #[test_case("81", ""; "bare definite")]
    #[test_case("9f", "ff"; "bare indefinite")]
    #[test_case("d9010281", ""; "tagged definite")]
    #[test_case("d901029f", "ff"; "tagged indefinite")]
    fn singleton_witnesses_decode_at_both_versions(prefix: &str, suffix: &str) {
        deduplicated_witness_values().into_iter().chain(plutus_script_values()).for_each(|(field, value)| {
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
        deduplicated_witness_values().into_iter().chain(plutus_script_values()).for_each(|(field, _)| {
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
    /// definite-length array, an indefinite-length array, or the `#6.258(...)` form the Conway CDDL
    /// prescribes. All three must decode, and all three re-encode to the tagged definite form. That
    /// is why a block that carries either lenient shape never reproduces its own bytes.
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
