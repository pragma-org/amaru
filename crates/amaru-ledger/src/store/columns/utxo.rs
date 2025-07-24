// Copyright 2024 PRAGMA
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

use amaru_kernel::{MemoizedTransactionOutput, TransactionInput};
use iter_borrow::IterBorrow;

pub type Key = TransactionInput;

pub type Value = MemoizedTransactionOutput;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Value>>;

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::store::minicbor;
    use amaru_kernel::{
        Address, Bytes, Constr, Hash, Int, KeepRaw, MaybeIndefArray, MemoizedDatum,
        MemoizedPlutusData, MemoizedScript, Network, PlutusData, PlutusScript,
        PostAlonzoTransactionOutput, PseudoTransactionOutput, ShelleyAddress,
        ShelleyDelegationPart, ShelleyPaymentPart, Value as KernelValue,
    };
    use proptest::option;
    use proptest::prelude::*;

    prop_compose! {
        pub fn any_txin()(
            hash in any::<[u8; 32]>(),
            index in any::<u16>(),
        ) -> TransactionInput {
            TransactionInput {
                transaction_id: Hash::from(hash),
                index: index as u64,
            }
        }
    }

    pub fn any_pseudo_transaction_output(
    ) -> impl Strategy<Value = PseudoTransactionOutput<PostAlonzoTransactionOutput>> {
        any::<u64>().prop_map(|amount| {
            let inner = PostAlonzoTransactionOutput {
                address: Bytes::from(vec![0u8; 32]),
                value: KernelValue::Coin(amount),
                datum_option: None,
                script_ref: None,
            };
            PseudoTransactionOutput::PostAlonzo(inner)
        })
    }

    pub fn any_memoized_plutus_script() -> impl Strategy<Value = MemoizedScript> {
        prop_oneof![
            prop::collection::vec(any::<u8>(), 1..128).prop_map(|bytes| {
                MemoizedScript::PlutusV1Script(PlutusScript::<1>(Bytes::from(bytes)))
            }),
            prop::collection::vec(any::<u8>(), 1..128).prop_map(|bytes| {
                MemoizedScript::PlutusV2Script(PlutusScript::<2>(Bytes::from(bytes)))
            }),
            prop::collection::vec(any::<u8>(), 1..128).prop_map(|bytes| {
                MemoizedScript::PlutusV3Script(PlutusScript::<3>(Bytes::from(bytes)))
            }),
        ]
    }

    fn any_hash28() -> impl Strategy<Value = Hash<28>> {
        prop::collection::vec(any::<u8>(), 28).prop_map(|v| {
            let mut bytes = [0u8; 28];
            bytes.copy_from_slice(&v);
            Hash::new(bytes)
        })
    }

    pub fn any_shelley_address() -> impl Strategy<Value = Address> {
        (any::<bool>(), any_hash28(), any_hash28()).prop_map(
            |(is_mainnet, payment_hash, delegation_hash)| {
                let network = if is_mainnet {
                    Network::Mainnet
                } else {
                    Network::Testnet
                };

                let payment = ShelleyPaymentPart::Key(payment_hash);
                let delegation = ShelleyDelegationPart::Key(delegation_hash);

                Address::Shelley(ShelleyAddress::new(network, payment, delegation))
            },
        )
    }

    pub fn any_value() -> impl Strategy<Value = amaru_kernel::Value> {
        any::<u64>().prop_map(|c| amaru_kernel::Value::Coin(c))
    }

    pub fn any_memoized_inline_datum() -> impl Strategy<Value = MemoizedDatum> {
        any::<i64>().prop_map(|n| {
            let int_val: Int = n.into();
            let big_int = amaru_kernel::BigInt::Int(int_val);

            let pd = PlutusData::Constr(Constr {
                tag: 121,
                any_constructor: None,
                fields: MaybeIndefArray::Def(vec![PlutusData::BigInt(big_int)]),
            });

            let mut buf = Vec::new();
            minicbor::encode(&pd, &mut buf).unwrap();

            let raw = Bytes::from(buf);
            let keep_raw = KeepRaw::new(raw.as_ref(), pd);

            MemoizedDatum::Inline(MemoizedPlutusData::from(keep_raw))
        })
    }

    pub fn any_memoized_transaction_output() -> impl Strategy<Value = MemoizedTransactionOutput> {
        (
            any_shelley_address(),
            any_value(),
            option::of(any_memoized_inline_datum()),
            option::of(any_memoized_plutus_script()),
        )
            .prop_map(|(address, value, datum_opt, script)| {
                let datum = datum_opt.unwrap_or(MemoizedDatum::None);

                let is_legacy = matches!(datum, MemoizedDatum::None) && script.is_none();

                MemoizedTransactionOutput {
                    is_legacy,
                    address,
                    value,
                    datum,
                    script,
                }
            })
    }
}
