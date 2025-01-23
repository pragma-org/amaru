use crate::{
    iter::borrow as iter_borrow,
    kernel::{Epoch, PoolId, PoolParams},
};
use pallas_codec::minicbor::{self as cbor};
use tracing::debug;

pub const EVENT_TARGET: &str = "amaru::ledger::store::pools";

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (PoolParams, Epoch);

pub type Key = PoolId;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub current_params: PoolParams,
    pub future_params: Vec<(Option<PoolParams>, Epoch)>,
}

impl Row {
    pub fn new(current_params: PoolParams) -> Self {
        Self {
            current_params,
            future_params: Vec::new(),
        }
    }

    /// Alter a Pool object by applying updates recorded across the epoch. A pool can have two types of
    /// updates:
    ///
    /// 1. Re-registration (effectively adjusting its underlying parameters), which always take effect
    ///    on the next epoch boundary.
    ///
    /// 2. Retirements, which specifies an epoch where the retirement becomes effective.
    ///
    /// While we collect all updates as they arrive from blocks, a few rules apply:
    ///
    /// a. Any re-registration that comes after a retirement cancels that retirement.
    /// b. Any retirement that come after a retirement cancels that initial retirement.
    pub fn tick<'a>(
        mut row: Box<dyn std::borrow::BorrowMut<Option<Self>> + 'a>,
        current_epoch: Epoch,
    ) {
        let (update, retirement, needs_update) = match row.borrow().as_ref() {
            None => (None, None, false),
            Some(pool) => pool.fold_future_params(current_epoch),
        };

        if needs_update {
            // This drops the immutable borrow. We avoid cloning inside the fold because we only ever need
            // to clone the last update. Yet we can't hold onto a reference because we must acquire a
            // mutable borrow below.
            let update: Option<PoolParams> = update.cloned();

            let pool: &mut Option<Row> = row.borrow_mut();

            // If the most recent retirement is effective as per the current epoch, we simply drop the
            // entry. Note that, any re-registration happening after that retirement would cancel it,
            // which is taken care of in the fold above (returning 'None').
            if let Some(epoch) = retirement {
                if epoch <= current_epoch {
                    debug!(
                        target: EVENT_TARGET,
                        pool = %pool
                            .as_ref()
                            .unwrap_or_else(|| unreachable!("pre-condition: needs_update"))
                            .current_params
                            .id,
                        "tick.retiring"
                    );

                    // NOTE:
                    // Callee shall ensure that all pools are ticked on epoch-boundaries.
                    //
                    // Hence, since:
                    //
                    // 1. Re-registrations can only be scheduled for next epoch;
                    // 2. Re-registrations cancel out any retirement for the same epoch;
                    // 3. Retirements cancel out any retirement scheduled and not yet enacted.
                    //
                    // Then we cannot find a case where a pool retires and still have a
                    // re-registration or another retirement still scheduled. Note that the reason
                    // we enforce this invariant here is because the next action will erase the
                    // pool -- and any remaining updates with it. This would have dramatic
                    // consequences should we still have updates stashed for the future.
                    let last = pool
                        .as_ref()
                        .unwrap_or_else(|| unreachable!("pre-condition: needs_update"))
                        .future_params
                        .last();
                    assert_eq!(
                        last,
                        Some(&(None, current_epoch)),
                        "invariant violation: most recent retirement is not last certificate: {:?}",
                        last,
                    );
                    *pool = None;
                    return;
                }
            }

            // Unwrap is safe here because we know the entry exists. Otherwise we wouldn't have got an
            // update to begin with!
            let pool = pool
                .as_mut()
                .unwrap_or_else(|| unreachable!("pre-condition: needs_update"));

            if let Some(new_params) = update {
                debug!(
                    target: EVENT_TARGET,
                    pool = %pool.current_params.id,
                    ?new_params,
                    "tick.updating"
                );
                pool.current_params = new_params;
            }

            // Regardless, always prune future params from those that are now-obsolete.
            pool.future_params
                .retain(|(_, epoch)| epoch > &current_epoch);
        }
    }

    /// Collapse stake pool future parameters according to the current epoch. The stable DB is at most k
    /// blocks in the past. So, if a certificate is submitted near the end (i.e. within k blocks) of the
    /// last epoch, then we could be in a situation where we haven't yet processed the registrations
    /// (since they're processed with a delay of k blocks) but have already moved into the next epoch.
    ///
    /// The function returns any new params becoming active in the 'current_epoch', and the retirement
    /// status of the pool. Note that the pool can both have new parameters AND a retirement scheduled
    /// at a later epoch.
    ///
    /// The boolean indicates whether any of the future params are now-obsolete as per the
    /// 'current_epoch'.
    fn fold_future_params(
        &self,
        current_epoch: Epoch,
    ) -> (Option<&PoolParams>, Option<Epoch>, bool) {
        self.future_params.iter().fold(
            (None, None, false),
            |(update, retirement, any_now_obsolete), (params, epoch)| match params {
                Some(params) if epoch <= &current_epoch => (Some(params), None, true),
                None => {
                    if epoch <= &current_epoch {
                        (None, Some(*epoch), true)
                    } else {
                        (update, Some(*epoch), any_now_obsolete)
                    }
                }
                Some(..) => (update, retirement, any_now_obsolete),
            },
        )
    }

    pub fn extend(mut bytes: Vec<u8>, future_params: (Option<PoolParams>, Epoch)) -> Vec<u8> {
        let tail = bytes.split_off(bytes.len() - 1);
        assert_eq!(tail, vec![0xFF], "invalid pool tail");
        cbor::encode(future_params, &mut bytes)
            .unwrap_or_else(|e| panic!("unable to encode pool params to CBOR: {e:?}"));
        [bytes, tail].concat()
    }

    pub fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode pool from CBOR ({}): {e:?}",
                hex::encode(&bytes)
            )
        })
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
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
        let current_params = d.decode_with(ctx)?;

        let mut iter = d.array_iter()?;

        let mut future_params = Vec::new();
        for item in &mut iter {
            future_params.push(item?);
        }

        Ok(Row {
            current_params,
            future_params,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{Hash, Nullable, RationalNumber};
    use proptest::prelude::*;

    prop_compose! {
        fn any_pool_params()(
            id in any::<[u8; 28]>(),
            vrf in any::<[u8; 32]>(),
            pledge in any::<u64>(),
            cost in any::<u64>(),
            margin in 0..100u64,
            reward_account in any::<[u8; 28]>(),
        ) -> PoolParams {
            PoolParams {
                id: Hash::new(id),
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
    fn any_row() -> impl Strategy<Value = Row> {
        prop::collection::vec(0..3u64, 0..3)
            .prop_flat_map(|epochs| {
                epochs
                    .into_iter()
                    .map(any_future_params)
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
                            (1..3u64).prop_map(move |offset| (None, epoch as u64 + offset)),
                            any_pool_params()
                                .prop_map(move |params| (Some(params), epoch as u64 + 1))
                        ]
                    };
                    prop::collection::vec(future_params(), 0..3)
                })
                .collect::<Vec<_>>()
        })
    }

    proptest! {
        #[test]
        fn prop_roundtrip_cbor(row in any_row()) {
            let mut bytes = Vec::new();
            cbor::encode(&row, &mut bytes)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
            assert_eq!(Ok(row), cbor::decode(&bytes).map_err(|e| e.to_string()));
        }
    }

    proptest! {
        #[test]
        fn prop_decode_after_extend(row in any_row(), future_params in any_future_params(100)) {
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
                    if retirement <= current_epoch as u64 {
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
                Row::tick(Box::new(&mut row), current_epoch as Epoch);
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
                            row.future_params.iter().filter(|(_, epoch)| epoch <= &(current_epoch as u64)).count() == 0,
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
