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
pub(crate) mod test {
    use amaru_kernel::{prop_cbor_roundtrip, DRep, Hash, Lovelace, StakeCredential};
    use amaru_ledger::store::columns::accounts::Row;
    use proptest::{option, prelude::*};

    use crate::test_utils::{dreps::tests::any_certificate_pointer, pools::tests::any_pool_id};

    prop_cbor_roundtrip!(Row, any_row());

    prop_compose! {
        pub(crate) fn any_row()(
            delegatee in option::of(any_pool_id()),
            deposit in any::<Lovelace>(),
            drep in option::of(any_drep()),
            drep_registered_at in any_certificate_pointer(),
            rewards in any::<Lovelace>(),
        ) -> Row {
            Row {
                delegatee,
                deposit,
                drep: drep.map(|drep| (drep, drep_registered_at)),
                rewards,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_drep()(
            credential in any::<[u8; 28]>(),
            kind in any::<u8>(),
        ) -> DRep {
            let kind = kind % 4;
            match kind {
                0 => DRep::Key(Hash::from(credential)),
                1 => DRep::Script(Hash::from(credential)),
                2 => DRep::Abstain,
                3 => DRep::NoConfidence,
                _ => unreachable!("% 4")
            }

        }
    }

    pub(crate) fn any_stake_credential() -> impl Strategy<Value = StakeCredential> {
        prop_oneof![
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::AddrKeyhash(Hash::new(hash))),
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::ScriptHash(Hash::new(hash))),
        ]
    }
}
