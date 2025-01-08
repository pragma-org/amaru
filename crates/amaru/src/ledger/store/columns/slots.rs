/// This modules captures blocks made by slot leaders throughout epochs.
use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{PoolId, Slot},
};
use pallas_codec::minicbor::{self as cbor};

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Slot, Option<Row>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub slot_leader: PoolId,
}

impl Row {
    pub fn new(slot_leader: PoolId) -> Self {
        Self { slot_leader }
    }

    fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode slot leader from CBOR ({}): {e:?}",
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
        e.encode_with(self.slot_leader, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let slot_leader = d.decode_with(ctx)?;
        Ok(Row::new(slot_leader))
    }
}

pub mod rocksdb {
    use super::Row;
    use crate::ledger::{
        kernel::Slot,
        store::rocksdb::common::{as_key, as_value, PREFIX_LEN},
    };
    use rocksdb::{self, OptimisticTransactionDB, ThreadMode, Transaction};

    /// Name prefixed used for storing Pool entries. UTF-8 encoding for "slot"
    pub const PREFIX: [u8; PREFIX_LEN] = [0x73, 0x6c, 0x6f, 0x74];

    pub fn get<T: ThreadMode>(
        db: &OptimisticTransactionDB<T>,
        slot: &Slot,
    ) -> Result<Option<super::Row>, rocksdb::Error> {
        Ok(db
            .get(as_key(&PREFIX, slot))?
            .map(super::Row::unsafe_decode))
    }

    pub fn put<DB>(db: &Transaction<'_, DB>, slot: &Slot, row: Row) -> Result<(), rocksdb::Error> {
        db.put(as_key(&PREFIX, slot), as_value(row))
    }
}
