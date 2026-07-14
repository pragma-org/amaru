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
use amaru_kernel::{CertificatePointer, Epoch, Lovelace, PoolId, PoolParams, cbor};

pub const EVENT_TARGET: &str = "amaru::ledger::store::pools";

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (PoolParams, CertificatePointer, Lovelace, Epoch);

pub type Key = PoolId;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub registered_at: CertificatePointer,
    pub deposit: Lovelace,
    pub current_params: PoolParams,
    pub future_params: Vec<(Option<PoolParams>, Epoch)>,
}

impl Row {
    pub fn new(registered_at: CertificatePointer, deposit: Lovelace, current_params: PoolParams) -> Self {
        Self { registered_at, deposit, current_params, future_params: Vec::new() }
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
        e.array(4)?;
        e.encode_with(self.registered_at, ctx)?;
        e.encode_with(self.deposit, ctx)?;
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
        let deposit = d.decode_with(ctx)?;

        let current_params = d.decode_with(ctx)?;

        let mut iter = d.array_iter()?;

        let mut future_params = Vec::new();
        for item in &mut iter {
            future_params.push(item?);
        }

        Ok(Row { registered_at, deposit, current_params, future_params })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{any_certificate_pointer, any_lovelace, any_pool_params, prop_cbor_roundtrip};
    use proptest::{collection, prelude::*};

    use super::*;

    pub fn any_future_params(epoch: Epoch) -> impl Strategy<Value = (Option<PoolParams>, Epoch)> {
        prop_oneof![Just((None, epoch)), any_pool_params().prop_map(move |params| (Some(params), epoch))]
    }

    // Generate arbitrary `Row`, good for serialization for not for logic.
    pub fn any_row() -> impl Strategy<Value = Row> {
        let any_future_params = collection::vec(0..3u64, 0..3)
            .prop_flat_map(|epochs| epochs.into_iter().map(|u| any_future_params(Epoch::from(u))).collect::<Vec<_>>());

        (any_future_params, any_pool_params(), any_certificate_pointer(u64::MAX), any_lovelace()).prop_map(
            |(future_params, current_params, registered_at, deposit)| Row {
                current_params,
                future_params,
                registered_at,
                deposit,
            },
        )
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
    }
}
