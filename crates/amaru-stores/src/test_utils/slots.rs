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
    use amaru_kernel::{PoolId, Slot};
    use amaru_ledger::store::columns::slots::Row;
    use proptest::prelude::*;

    prop_compose! {
        pub(crate) fn any_slot()(
            n in 0u64..87782400,
        ) -> Slot {
            Slot::from(n)
        }
    }

    prop_compose! {
        pub(crate) fn any_row()(
            slot_leader in any::<[u8; 28]>().prop_map(PoolId::new),
        ) -> Row {
            Row::new(slot_leader)
        }
    }
}
