use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{Epoch, PoolParams},
};
use pallas_codec::minicbor::{self as cbor};

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified
/// imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Option<Row>>;

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
    pub fn fold_future_params(
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
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        [bytes, tail].concat()
    }

    fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes)
            .unwrap_or_else(|e| panic!("unable to decode pool ({}): {e:?}", hex::encode(&bytes)))
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

pub mod rocksdb {
    use crate::ledger::{
        kernel::{Epoch, PoolId, PoolParams},
        store::rocksdb::common::{as_key, as_value, PREFIX_LEN},
    };
    use rocksdb::{self, OptimisticTransactionDB, ThreadMode, Transaction};
    use tracing::{debug, warn};

    /// Name prefixed used for storing Pool entries. UTF-8 encoding for "pool"
    pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x6f, 0x6c];

    pub fn get<T: ThreadMode>(
        db: &OptimisticTransactionDB<T>,
        pool: &PoolId,
    ) -> Result<Option<super::Row>, rocksdb::Error> {
        Ok(db
            .get(as_key(&PREFIX, pool))?
            .map(super::Row::unsafe_decode))
    }

    pub fn add<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = (PoolParams, Epoch)>,
    ) -> Result<(), rocksdb::Error> {
        for (params, epoch) in rows {
            let pool = params.id;

            // Pool parameters are stored in an epoch-aware fashion.
            //
            // - If no parameters exist for the pool, we can immediately create a new
            //   entry.
            //
            // - If one already exists, then the parameters are stashed until the next
            //   epoch boundary.
            //
            // TODO: We might want to define a MERGE OPERATOR to speed this up if
            // necessary.
            let params = match db.get(as_key(&PREFIX, pool))? {
                None => {
                    debug!(?pool, "insert");
                    as_value(super::Row::new(params))
                }
                Some(existing_params) => {
                    debug!(?pool, scheduled_for_epoch=?epoch, "extend");
                    super::Row::extend(existing_params, (Some(params), epoch))
                }
            };

            db.put(as_key(&PREFIX, pool), params)?;
        }

        Ok(())
    }

    pub fn remove<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = (PoolId, Epoch)>,
    ) -> Result<(), rocksdb::Error> {
        for (pool, epoch) in rows {
            debug!(?pool, scheduled_for_epoch=?epoch, "remove");
            // We do not delete pool immediately but rather schedule the
            // removal as an empty parameter update. The 'pool reaping' happens on
            // every epoch boundary.
            match db.get(as_key(&PREFIX, pool))? {
                None => {
                    warn!(?pool, "attempting to remove non existing pool")
                }
                Some(existing_params) => db.put(
                    as_key(&PREFIX, pool),
                    super::Row::extend(existing_params, (None, epoch)),
                )?,
            };
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::kernel::{Hash, Nullable, RationalNumber};
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

    fn any_row() -> impl Strategy<Value = Row> {
        any::<Vec<Epoch>>()
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

    proptest! {
        #[test]
        fn prop_roundtrip_cbor(row in any_row()) {
            let mut bytes = Vec::new();
            cbor::encode(&row, &mut bytes)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
            assert_eq!(Ok(row), cbor::decode(&bytes).map_err(|e| e.to_string()));
        }

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
}
