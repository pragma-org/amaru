use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{Lovelace, PoolId, StakeCredential},
};
use pallas_codec::minicbor::{self as cbor};

/// Iterator used to browse rows from the Accounts column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Option<Row>>;

pub type Add = (StakeCredential, Option<PoolId>, Lovelace, Lovelace);

pub type Remove = StakeCredential;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub delegatee: Option<PoolId>,
    pub deposit: Lovelace,
    // FIXME: We probably want to use an arbitrarily-sized for rewards; Going
    // for a Lovelace (aliasing u64) for now as we are only demonstrating the
    // ledger-state storage capabilities and it doesn't *fundamentally* change
    // anything.
    pub rewards: Lovelace,
}

impl Row {
    fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode account from CBOR ({}): {e:?}",
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
        e.encode_with(self.delegatee, ctx)?;
        e.encode_with(self.deposit, ctx)?;
        e.encode_with(self.rewards, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            delegatee: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            rewards: d.decode_with(ctx)?,
        })
    }
}

pub mod rocksdb {
    use super::Row;
    use crate::ledger::store::rocksdb::common::{as_key, as_value, PREFIX_LEN};
    use rocksdb::{self, Transaction};

    /// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
    pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

    /// Register a new credential, with or without a stake pool.
    pub fn add<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = super::Add>,
    ) -> Result<(), rocksdb::Error> {
        for (credential, delegatee, deposit, rewards) in rows {
            let key = as_key(&PREFIX, credential);

            // In case where a registration already exists, then we must only update the underlying
            // entry, while preserving the reward amount.
            let row = if let Some(mut row) = db.get(&key)?.map(Row::unsafe_decode) {
                row.delegatee = delegatee;
                row.deposit = deposit;
                row
            } else {
                Row {
                    delegatee,
                    deposit,
                    rewards,
                }
            };

            db.put(key, as_value(row))?;
        }

        Ok(())
    }

    /// Clear a stake credential registration.
    pub fn remove<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = super::Remove>,
    ) -> Result<(), rocksdb::Error> {
        for credential in rows {
            db.delete(as_key(&PREFIX, &credential))?;
        }

        Ok(())
    }
}
