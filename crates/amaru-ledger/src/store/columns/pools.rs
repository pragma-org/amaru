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
use amaru_kernel::{CertificatePointer, Lovelace, PoolId, PoolParams, cbor};

use crate::epoch_transition::pools_updates::{PoolCertificate, PoolCertificates};

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (PoolParams, CertificatePointer, Lovelace);

pub type Key = PoolId;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub registered_at: CertificatePointer,
    pub deposit: Lovelace,
    pub current_params: PoolParams,
    pub pending_certificates: PoolCertificates,
}

impl Row {
    pub fn new(registered_at: CertificatePointer, deposit: Lovelace, current_params: PoolParams) -> Self {
        Self { registered_at, deposit, current_params, pending_certificates: Default::default() }
    }

    /// Returns the pool id
    pub fn id(&self) -> PoolId {
        self.current_params.id
    }

    #[expect(clippy::panic)]
    pub fn extend(mut bytes: Vec<u8>, certificate: PoolCertificate) -> Vec<u8> {
        let tail = bytes.split_off(bytes.len() - 1);
        assert_eq!(tail, vec![0xFF], "invalid pool tail");
        cbor::encode(certificate, &mut bytes).unwrap_or_else(|e| panic!("unable to encode pool params to CBOR: {e:?}"));
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
        e.encode_with(&self.pending_certificates, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let registered_at = d.decode_with(ctx)?;
        let deposit = d.decode_with(ctx)?;
        let current_params = d.decode_with(ctx)?;
        let pending_certificates = d.decode_with(ctx)?;
        Ok(Row { registered_at, deposit, current_params, pending_certificates })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{any_certificate_pointer, any_lovelace, any_pool_params, prop_cbor_roundtrip};
    use proptest::prelude::*;

    use super::*;
    #[cfg(test)]
    use crate::epoch_transition::pools_updates::any_pool_certificate;
    use crate::epoch_transition::pools_updates::any_pool_certificates;

    // Generate arbitrary `Row`, good for serialization for not for logic.
    pub fn any_row() -> impl Strategy<Value = Row> {
        (any_pool_params(), any_pool_certificates(), any_certificate_pointer(u64::MAX), any_lovelace()).prop_map(
            |(current_params, pending_certificates, registered_at, deposit)| Row {
                current_params,
                pending_certificates,
                registered_at,
                deposit,
            },
        )
    }

    prop_cbor_roundtrip!(Row, any_row());

    proptest! {
        #[test]
        fn prop_decode_after_extend(row in any_row(), certificate in any_pool_certificate(amaru_kernel::Epoch::from(100))) {
            let mut bytes = Vec::new();
            cbor::encode(&row, &mut bytes)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));

            let bytes_extended = Row::extend(bytes, certificate.clone());
            let row_extended: Row = cbor::decode(&bytes_extended).unwrap();

            let mut pending_certificates = row.pending_certificates.clone();
            pending_certificates.append_certificate(certificate);
            let expected = Row {
                pending_certificates,
                ..row
            };

            prop_assert_eq!(row_extended, expected);
        }
    }
}
