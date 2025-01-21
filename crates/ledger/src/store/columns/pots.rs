/// This modules captures protocol-wide value pots such as treasury and reserves accounts.
use crate::kernel::Lovelace;
use pallas_codec::minicbor::{self as cbor};

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub fees: Lovelace,
}

impl Row {
    pub fn new(treasury: Lovelace, reserves: Lovelace, fees: Lovelace) -> Self {
        Self {
            treasury,
            reserves,
            fees,
        }
    }

    pub fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode pots from CBOR ({}): {e:?}",
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
        e.array(3)?;
        e.encode_with(self.treasury, ctx)?;
        e.encode_with(self.reserves, ctx)?;
        e.encode_with(self.fees, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        let treasury = d.decode_with(ctx)?;
        let reserves = d.decode_with(ctx)?;
        let fees = d.decode_with(ctx)?;
        Ok(Row::new(treasury, reserves, fees))
    }
}
