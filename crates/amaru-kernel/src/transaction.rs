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
    AuxiliaryData, Debug, KeepRaw, MintedWitnessSet, Nullable, TransactionBody, WitnessSet,
    cbor::{Decode, Encode},
};

#[derive(Debug, Clone, Encode, Decode)]
pub struct PseudoTx<T2, T3>
where
    T2: std::clone::Clone,
    T3: std::clone::Clone,
{
    #[n(0)]
    pub body: TransactionBody,

    #[n(1)]
    pub witness_set: T2,

    #[n(2)]
    pub success: bool,

    #[n(3)]
    pub auxiliary_data: Nullable<T3>,
}

pub type Tx = PseudoTx<WitnessSet, AuxiliaryData>;

pub type MintedTx<'b> = PseudoTx<KeepRaw<'b, MintedWitnessSet<'b>>, KeepRaw<'b, AuxiliaryData>>;

impl<'b> From<MintedTx<'b>> for Tx {
    fn from(x: MintedTx<'b>) -> Self {
        Tx {
            body: x.body,
            witness_set: x.witness_set.unwrap().into(),
            success: x.success,
            auxiliary_data: x.auxiliary_data.map(|x| x.unwrap()),
        }
    }
}
