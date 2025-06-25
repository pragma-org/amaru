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

#[cfg(test)]
pub(crate) mod tests {

    use amaru_kernel::{
        prop_cbor_roundtrip, Anchor, CertificatePointer, Hash, Lovelace, Slot, TransactionPointer,
    };
    use amaru_ledger::store::columns::dreps::Row;
    use proptest::{option, prelude::*, string};

    use crate::test_utils::proposals::tests::any_epoch;

    prop_cbor_roundtrip!(Row, any_row());

    prop_compose! {
        pub fn any_row()(
            deposit in any::<Lovelace>(),
            anchor in option::of(any_anchor()),
            registered_at in any_certificate_pointer(),
            last_interaction in option::of(any_epoch()),
            previous_deregistration in option::of(any_certificate_pointer()),
        ) -> Row {
            Row {
                deposit,
                anchor,
                registered_at,
                last_interaction,
                previous_deregistration,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_transaction_pointer()(
            slot in 518_400u64..600_000,
            transaction_index in any::<usize>(),
        ) -> TransactionPointer {
            TransactionPointer {
                slot: Slot::from(slot),
                transaction_index,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_certificate_pointer()(
            transaction in any_transaction_pointer(),
            certificate_index in any::<usize>(),
        ) -> CertificatePointer {
            CertificatePointer {
                transaction,
                certificate_index,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_anchor()(
            // NOTE: This can be any string really, but it's cute to generate URLs, isn't it?
            url in string::string_regex("(https:)?[a-zA-Z0-9]{2,}(\\.[a-zA-Z0-9]{2,})(\\.[a-zA-Z0-9]{2,})?").unwrap(),
            content_hash in any::<[u8; 32]>(),
        ) -> Anchor {
            Anchor {
                url,
                content_hash: Hash::from(content_hash),
            }
        }

    }
}
