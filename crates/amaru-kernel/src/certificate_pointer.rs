// Copyright 2025 PRAGMA
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

use crate::{Slot, TransactionPointer, cbor, heterogeneous_array};
use std::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd)]
pub struct CertificatePointer {
    pub transaction: TransactionPointer,
    pub certificate_index: usize,
}

impl CertificatePointer {
    pub fn slot(&self) -> Slot {
        self.transaction.slot
    }
}

impl fmt::Display for CertificatePointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{},certificate={}",
            &self.transaction, &self.certificate_index
        )
    }
}

impl<C> cbor::encode::Encode<C> for CertificatePointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.transaction, ctx)?;
        e.encode_with(self.certificate_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for CertificatePointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(CertificatePointer {
                transaction: d.decode_with(ctx)?,
                certificate_index: d.decode_with(ctx)?,
            })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::{prop_cbor_roundtrip, tests::any_transaction_pointer};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(CertificatePointer, any_certificate_pointer(u64::MAX));

    prop_compose! {
        pub fn any_certificate_pointer(max_slot: u64)(
            transaction in any_transaction_pointer(max_slot),
            certificate_index in any::<usize>(),
        ) -> CertificatePointer {
            CertificatePointer {
                transaction,
                certificate_index,
            }
        }
    }

    #[cfg(test)]
    mod internal {
        use super::*;
        use test_case::test_case;

        #[test_case((42, 0, 0), (42, 0, 0) => with |(left, right)| assert_eq!(left, right); "reflexivity")]
        #[test_case((42, 0, 0), (43, 0, 0) => with |(left, right)| assert!(left < right); "across slots")]
        #[test_case((42, 0, 0), (42, 1, 0) => with |(left, right)| assert!(left < right); "across transactions")]
        #[test_case((42, 0, 0), (42, 0, 1) => with |(left, right)| assert!(left < right); "across certificates")]
        #[test_case((42, 0, 5), (42, 1, 0) => with |(left, right)| assert!(left < right); "across transactions and certs")]
        fn test_pointers(
            left: (u64, usize, usize),
            right: (u64, usize, usize),
        ) -> (CertificatePointer, CertificatePointer) {
            let new_pointer = |args: (Slot, usize, usize)| CertificatePointer {
                transaction: TransactionPointer {
                    slot: args.0,
                    transaction_index: args.1,
                },
                certificate_index: args.2,
            };

            (
                new_pointer((Slot::from(left.0), left.1, left.2)),
                new_pointer((Slot::from(right.0), right.1, right.2)),
            )
        }
    }
}
