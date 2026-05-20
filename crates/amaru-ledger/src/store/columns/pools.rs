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

use amaru_iter_borrow::IterBorrow;
use amaru_kernel::{CertificatePointer, Epoch, PoolId, PoolParams, cbor};

pub const EVENT_TARGET: &str = "amaru::ledger::store::pools";

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (PoolParams, CertificatePointer, Epoch);

pub type Key = PoolId;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub registered_at: CertificatePointer,
    pub current_params: PoolParams,
    pub future_params: Vec<(Option<PoolParams>, Epoch)>,
}

impl Row {
    pub fn new(registered_at: CertificatePointer, current_params: PoolParams) -> Self {
        Self { registered_at, current_params, future_params: Vec::new() }
    }

    /// Returns the pool id
    pub fn id(&self) -> PoolId {
        self.current_params.id
    }

    #[expect(clippy::panic)]
    pub fn extend(mut bytes: Vec<u8>, future_params: (Option<PoolParams>, Epoch)) -> Vec<u8> {
        let tail = bytes.split_off(bytes.len() - 1);
        assert_eq!(tail, vec![0xFF], "invalid pool tail");
        cbor::encode(future_params, &mut bytes)
            .unwrap_or_else(|e| panic!("unable to encode pool params to CBOR: {e:?}"));
        [bytes, tail].concat()
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.encode_with(self.registered_at, ctx)?;
        e.encode_with(&self.current_params, ctx)?;
        // NOTE: We explicitly enforce the use of *indefinite* arrays here because it allows us
        // to extend the serialized data easily without having to deserialise it.
        e.begin_array()?;
        for update in self.future_params.iter() {
            e.encode_with(update, ctx)?;
        }
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let registered_at = d.decode_with(ctx)?;

        let current_params = d.decode_with(ctx)?;

        let mut iter = d.array_iter()?;

        let mut future_params = Vec::new();
        for item in &mut iter {
            future_params.push(item?);
        }

        Ok(Row { registered_at, current_params, future_params })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{any_certificate_pointer, any_pool_params, prop_cbor_roundtrip};
    use proptest::{collection, collection::vec, prelude::*};

    use super::*;

    pub fn any_future_params(epoch: Epoch) -> impl Strategy<Value = (Option<PoolParams>, Epoch)> {
        prop_oneof![Just((None, epoch)), any_pool_params().prop_map(move |params| (Some(params), epoch))]
    }

    // Generate arbitrary `Row`, good for serialization for not for logic.
    pub fn any_row() -> impl Strategy<Value = Row> {
        let any_future_params = collection::vec(0..3u64, 0..3)
            .prop_flat_map(|epochs| epochs.into_iter().map(|u| any_future_params(Epoch::from(u))).collect::<Vec<_>>());

        (any_future_params, any_pool_params(), any_certificate_pointer(u64::MAX)).prop_map(
            |(future_params, current_params, registered_at)| Row { current_params, future_params, registered_at },
        )
    }

    // Generate a sequence of plausible updates, where each item in the vector correspond to an
    // epoch's update. So a caller is expected to tick a base Row between each application.
    pub fn any_row_seq_updates() -> impl Strategy<Value = Vec<Vec<(Option<PoolParams>, Epoch)>>> {
        vec(Just(()), 0..10).prop_flat_map(|cols| {
            cols.iter()
                .enumerate()
                .map(|(epoch, _)| {
                    let future_params = || {
                        prop_oneof![
                            (1..3u64).prop_map(move |offset| (None, Epoch::from(epoch as u64) + offset)),
                            any_pool_params().prop_map(move |params| (Some(params), Epoch::from(epoch as u64 + 1)))
                        ]
                    };
                    vec(future_params(), 0..3)
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

            prop_assert_eq!(row_extended.future_params.len(), row.future_params.len() + 1);
            prop_assert_eq!(row_extended.future_params.last(), Some(&future_params));
        }

        #[test]
        fn prop_tick_pool(
            registered_at in any_certificate_pointer(u64::MAX),
            initial_params in any_pool_params(),
            updates in any_row_seq_updates(),
        ) {
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

            let mut row = Some(Row::new(registered_at, initial_params));
            for (current_epoch, updates) in updates.into_iter().enumerate() {
                let current_epoch = Epoch::from(current_epoch as u64);
                // Apply model's changes at the epoch boundary
                if let Some(retirement) = model.retiring
                    && retirement <= current_epoch
                {
                    model.current = None;
                }
                if let Some(future) = model.future.take() {
                    model.current = Some(future);
                }

                // Process all updates through our simpler model
                model = updates.iter().fold(model, |mut model, (update, epoch)| {
                    match update {
                        None if model.current.is_none() => {},
                        None => { model.retiring = Some(*epoch); },
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
                Row::tick(Box::new(&mut row), current_epoch);
                match row.as_mut() {
                    None => {
                        if let Some(params) = updates.iter().find(|(params, _)| params.is_some()).cloned() {
                            let mut new = Row::new(registered_at, params.0.unwrap());
                            new.future_params.extend(updates.clone());
                            row = Some(new);
                        }
                    },
                    Some(row) => {
                        prop_assert_eq!(
                            model.current.as_ref(),
                            Some(&row.current_params),
                            "current_epoch = {:?}, model = {:?}",
                            current_epoch,
                            model
                        );

                        let obsolete_count = row.future_params.iter()
                            .filter(|(_, epoch)| epoch <= &current_epoch)
                            .count();
                        prop_assert_eq!(
                            obsolete_count,
                            0,
                            "future_params should not contain obsolete entries: {:?}",
                            row.future_params
                        );

                        row.future_params.extend(updates.clone());
                    }
                }
            }
        }
    }
}
