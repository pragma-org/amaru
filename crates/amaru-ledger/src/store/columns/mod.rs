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

pub mod accounts;
pub mod committees;
pub mod dreps;
pub mod pools;
pub mod pots;
pub mod slots;
pub mod utxo;

#[cfg(test)]
pub mod tests {
    use amaru_kernel::{CertificatePointer, Slot};
    use proptest::prelude::*;

    prop_compose! {
        pub fn any_certificate_pointer()(
            slot in any::<Slot>(),
            transaction_index in any::<usize>(),
            certificate_index in any::<usize>(),
        ) -> CertificatePointer {
            CertificatePointer {
                slot,
                transaction_index,
                certificate_index,
            }
        }
    }
}
