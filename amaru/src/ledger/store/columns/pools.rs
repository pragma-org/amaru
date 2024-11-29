use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{Epoch, PoolParams},
};
use pallas_codec::minicbor::{self as cbor};

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified
/// imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Option<Row>>;

#[derive(Debug, Clone)]
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

    pub fn extend(mut bytes: Vec<u8>, future_params: (Option<PoolParams>, Epoch)) -> Vec<u8> {
        let tail = bytes.split_off(bytes.len() - 1);
        assert_eq!(
            tail,
            vec![0xFF],
            "invalid tail of serialized pool parameters"
        );
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
                None => as_value(super::Row::new(params)),
                Some(existing_params) => super::Row::extend(existing_params, (Some(params), epoch)),
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
            // We do not delete pool immediately but rather schedule the
            // removal as an empty parameter update. The 'pool reaping' happens on
            // every epoch boundary.
            match db.get(as_key(&PREFIX, pool))? {
                None => (),
                Some(existing_params) => db.put(
                    as_key(&PREFIX, pool),
                    super::Row::extend(existing_params, (None, epoch)),
                )?,
            };
        }

        Ok(())
    }
}
