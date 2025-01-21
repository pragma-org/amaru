/// This modules captures blocks made by slot leaders throughout epochs.
use crate::{
    iter::borrow as iter_borrow,
    kernel::{PoolId, Slot},
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

    pub fn unsafe_decode(bytes: Vec<u8>) -> Self {
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
