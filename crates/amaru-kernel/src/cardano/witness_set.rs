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
    BootstrapWitness, MemoizedNativeScript, NonEmptyVec, PlutusDataSet, PlutusScript, Redeemers, VKeyWitness, cbor,
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
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    pub vkeywitness: Option<NonEmptyVec<VKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptyVec<MemoizedNativeScript>>,

    /// FIXME(cbor): Accidentally not a set
    ///
    /// See note on vkeywitness.
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::WitnessSet;
    use crate::{from_cbor_no_leftovers, to_cbor};

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const SIGNATURE: &str = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    /// A set of verification key witnesses arrives on-chain in any of three shapes: a bare
    /// definite-length array, an indefinite-length array, or the `#6.258(…)` form the Conway CDDL
    /// prescribes. All three must decode, and all three re-encode to the tagged definite form —
    /// which is why a block carrying either lenient shape never reproduces its own bytes.
    #[test_case("81", ""; "bare definite array, as found on-chain before Conway")]
    #[test_case("9f", "ff"; "indefinite-length array")]
    #[test_case("d9010281", ""; "tagged set, as the Conway CDDL prescribes")]
    fn vkey_witnesses_always_re_encode_as_a_tagged_definite_set(prefix: &str, suffix: &str) {
        let input = format!("a100{prefix}825820{KEY}5840{SIGNATURE}{suffix}");
        let expected = format!("a100d9010281825820{KEY}5840{SIGNATURE}");

        let witnesses: WitnessSet = from_cbor_no_leftovers(&hex::decode(&input).unwrap()).unwrap();

        let encoded = to_cbor(&witnesses);
        assert_eq!(hex::encode(&encoded), expected, "unexpected encoding");

        let re_decoded: WitnessSet = from_cbor_no_leftovers(&encoded).unwrap();
        assert_eq!(to_cbor(&re_decoded), encoded, "encoding is not a fixed point");
    }
}
