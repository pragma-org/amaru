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
    BootstrapWitness, MemoizedNativeScript, MemoizedPlutusData, NonEmptyVec, PlutusScript, Redeemers, VKeyWitness, cbor,
};

/// FIXME: Accidentally not a set
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

    /// FIXME: Accidentally not a set
    ///
    /// See note on vkeywitness.
    #[n(2)]
    pub bootstrap_witness: Option<NonEmptyVec<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptyVec<PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<NonEmptyVec<MemoizedPlutusData>>,

    #[n(5)]
    pub redeemer: Option<Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptyVec<PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptyVec<PlutusScript<3>>>,
}
