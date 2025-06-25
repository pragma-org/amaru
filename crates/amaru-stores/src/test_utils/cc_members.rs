#[cfg(test)]
pub mod test {
    use amaru_kernel::{prop_cbor_roundtrip, Hash, StakeCredential};
    use amaru_ledger::store::columns::cc_members::Row;
    use proptest::{option, prelude::*};

    prop_cbor_roundtrip!(Row, any_row());

    prop_compose! {
        pub fn any_row()(
            hot_credential in option::of(any_stake_credential()),
        ) -> Row {
            Row {
                hot_credential,
            }
        }
    }

    prop_compose! {
        pub fn any_stake_credential()(
            is_script in any::<bool>(),
            credential in any::<[u8; 28]>(),
        ) -> StakeCredential {
            if is_script {
                StakeCredential::ScriptHash(Hash::from(credential))
            } else {
                StakeCredential::AddrKeyhash(Hash::from(credential))
            }
        }
    }
}
