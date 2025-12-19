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
    AuxiliaryData, KeepRaw, MintedTransactionBody, MintedWitnessSet, TransactionBody, WitnessSet,
    cbor::{Decode, Encode},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Encode, Decode, Debug, PartialEq, Clone)]
pub struct PseudoBlock<T1, T2, T3, T4>
where
    T4: std::clone::Clone,
{
    #[n(0)]
    pub header: T1,

    #[b(1)]
    pub transaction_bodies: pallas_primitives::MaybeIndefArray<T2>,

    #[n(2)]
    pub transaction_witness_sets: pallas_primitives::MaybeIndefArray<T3>,

    #[n(3)]
    pub auxiliary_data_set:
        pallas_primitives::KeyValuePairs<pallas_primitives::TransactionIndex, T4>,

    #[n(4)]
    pub invalid_transactions:
        Option<pallas_primitives::MaybeIndefArray<pallas_primitives::TransactionIndex>>,
}

pub type Block =
    PseudoBlock<pallas_primitives::babbage::Header, TransactionBody, WitnessSet, AuxiliaryData>;

/// A memory representation of an already minted block
///
/// This structure is analogous to [Block], but it allows to retrieve the
/// original CBOR bytes for each structure that might require hashing. In this
/// way, we make sure that the resulting hash matches what exists on-chain.
pub type MintedBlock<'b> = PseudoBlock<
    KeepRaw<'b, pallas_primitives::babbage::MintedHeader<'b>>,
    KeepRaw<'b, MintedTransactionBody<'b>>,
    KeepRaw<'b, MintedWitnessSet<'b>>,
    KeepRaw<'b, AuxiliaryData>,
>;

impl<'b> From<MintedBlock<'b>> for Block {
    fn from(x: MintedBlock<'b>) -> Self {
        Block {
            header: x.header.unwrap().into(),
            transaction_bodies: pallas_primitives::MaybeIndefArray::Def(
                x.transaction_bodies
                    .iter()
                    .cloned()
                    .map(|x| x.unwrap())
                    .map(TransactionBody::from)
                    .collect(),
            ),
            transaction_witness_sets: pallas_primitives::MaybeIndefArray::Def(
                x.transaction_witness_sets
                    .iter()
                    .cloned()
                    .map(|x| x.unwrap())
                    .map(WitnessSet::from)
                    .collect(),
            ),
            auxiliary_data_set: x
                .auxiliary_data_set
                .to_vec()
                .into_iter()
                .map(|(k, v)| (k, v.unwrap()))
                .collect::<Vec<_>>()
                .into(),
            invalid_transactions: x.invalid_transactions,
        }
    }
}
