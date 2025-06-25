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

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{
        cbor, prop_cbor_roundtrip, Hash, Nullable, PoolId, PoolParams, RationalNumber,
    };
    use amaru_ledger::store::columns::pools::Row;
    use proptest::prelude::*;
    use slot_arithmetic::Epoch;

    prop_compose! {
        pub(crate) fn any_pool_entry()(
            pool_params in any_pool_params(),
            epoch in any::<u64>(),
        ) -> (PoolParams, Epoch) {
            (pool_params, Epoch::from(epoch))
        }
    }

    prop_compose! {
        pub(crate) fn any_pool_id()(
            bytes in any::<[u8; 28]>(),
        ) -> PoolId {
            Hash::from(bytes)
        }
    }

    prop_compose! {
        pub(crate) fn any_pool_params()(
            id in any_pool_id(),
            vrf in any::<[u8; 32]>(),
            pledge in any::<u64>(),
            cost in any::<u64>(),
            margin in 0..100u64,
            reward_account in any::<[u8; 28]>(),
        ) -> PoolParams {
            PoolParams {
                id,
                vrf: Hash::new(vrf),
                pledge,
                cost,
                margin: RationalNumber { numerator: margin, denominator: 100 },
                reward_account: [&[0xF0], &reward_account[..]].concat().into(),
                // TODO: Generate some arbitrary data
                owners: vec![].into(),
                relays: vec![],
                metadata: Nullable::Null,
            }
        }
    }

    fn any_future_params(epoch: Epoch) -> impl Strategy<Value = (Option<PoolParams>, Epoch)> {
        prop_oneof![
            Just((None, epoch)),
            any_pool_params().prop_map(move |params| (Some(params), epoch))
        ]
    }

    // Generate arbitrary `Row`, good for serialization for not for logic.
    pub fn any_row() -> impl Strategy<Value = Row> {
        prop::collection::vec(0..3u64, 0..3)
            .prop_flat_map(|epochs| {
                epochs
                    .into_iter()
                    .map(|u: u64| any_future_params(Epoch::from(u)))
                    .collect::<Vec<_>>()
            })
            .prop_flat_map(|future_params| {
                any_pool_params().prop_map(move |current_params| Row {
                    current_params,
                    future_params: future_params.clone(),
                })
            })
    }

    // Generate a sequence of plausible updates, where each item in the vector correspond to an
    // epoch's update. So a caller is expected to tick a base Row between each application.
    fn any_row_seq_updates() -> impl Strategy<Value = Vec<Vec<(Option<PoolParams>, Epoch)>>> {
        prop::collection::vec(Just(()), 0..10).prop_flat_map(|cols| {
            cols.iter()
                .enumerate()
                .map(|(epoch, _)| {
                    let future_params = || {
                        prop_oneof![
                            (1..3u64)
                                .prop_map(move |offset| (None, Epoch::from(epoch as u64) + offset)),
                            any_pool_params().prop_map(move |params| (
                                Some(params),
                                Epoch::from(epoch as u64 + 1)
                            ))
                        ]
                    };
                    prop::collection::vec(future_params(), 0..3)
                })
                .collect::<Vec<_>>()
        })
    }

    prop_cbor_roundtrip!(Row, any_row());

    proptest! {
        #[test]
        fn prop_decode_after_extend(row in any_row(), future_params in any_future_params(Epoch::from(100))) {
            let mut bytes = Vec::new();
            cbor::encode(&row, &mut bytes)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));

            let bytes_extended = Row::extend(bytes, future_params.clone());

            let row_extended: Row = cbor::decode(&bytes_extended).unwrap();

            assert_eq!(row_extended.future_params.len(), row.future_params.len() + 1);
            assert_eq!(row_extended.future_params.last(), Some(&future_params));
        }
    }

    proptest! {
        #[test]
        fn prop_tick_pool(initial_params in any_pool_params(), updates in any_row_seq_updates()) {
            #[derive(Debug)]
            struct Model {
                current: Option<PoolParams>,
                future: Option<PoolParams>,
                retiring: Option<Epoch>,
            }

            let mut model = Model {
                current: Some(initial_params.clone()),
                future: None,
                retiring: None,
            };

            let mut row = Some(Row::new(initial_params));
            for (current_epoch, updates) in updates.into_iter().enumerate() {
                // Apply model's changes at the epoch boundary
                if let Some(retirement) = model.retiring {
                    if retirement <= Epoch::from(current_epoch as u64) {
                        model.current = None;
                    }
                }
                if let Some(future) = model.future {
                    model.current = Some(future);
                }
                model.future = None;

                // Process all updates through our simpler model
                model = updates.iter().fold(model, |mut model, (update, epoch)| {
                    // Schedule or apply updates according to the current state
                    match update {
                        // NOTE: cannot happen in principle as the ledger rules forbids this.
                        // But our model is imperfect, so we simply ignore retirement when there's
                        // no pool.
                        None if model.current.is_none() => {},
                        None => {
                            model.retiring = Some(*epoch);
                        },
                        Some(params) if model.current.is_none() => {
                            model.retiring = None;
                            model.current = Some(params.clone());
                        },
                        Some(params) => {
                            model.retiring = None;
                            model.future = Some(params.clone());
                        },
                    }

                    model
                });

                // Process them through row ticks, and ensure conformance with the model
                Row::tick(Box::new(&mut row), Epoch::from(current_epoch as u64));
                match row.as_mut() {
                    None => {
                        // Re-register the pool if we end up de-registering it.
                        if let Some(params) = updates.iter().find(|(params, _)| params.is_some()).cloned() {
                            let mut new = Row::new(params.0.unwrap());
                            new.future_params.extend(updates);
                            row = Some(new);
                        }
                    },
                    Some(row) => {
                        assert_eq!(
                            model.current.as_ref(),
                            Some(&row.current_params),
                            "current_epoch = {current_epoch:?}, model = {model:?}",
                        );
                        assert!(
                            row.future_params.iter().filter(|(_, epoch)| epoch <= &(Epoch::from(current_epoch as u64))).count() == 0,
                            "future params = {:?}",
                            row.future_params,
                        );
                        row.future_params.extend(updates);
                    }
                }
            }
        }
    }
}
