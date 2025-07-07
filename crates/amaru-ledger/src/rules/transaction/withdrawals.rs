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

use crate::{
    context::{AccountsSlice, WitnessSlice},
    rules::TransactionField,
};
use amaru_kernel::{
    Address, HasOwnership, Lovelace, MemoizedDatum, RequiredScript, RewardAccount, ScriptPurpose,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidWithdrawals {
    #[error("unexpected bytes instead of reward account in {context:?} at position {position}")]
    MalformedRewardAccount {
        bytes: Vec<u8>,
        context: TransactionField,
        position: usize,
    },
}

pub(crate) fn execute<C>(
    context: &mut C,
    withdrawals: Option<&Vec<(RewardAccount, Lovelace)>>,
) -> Result<(), InvalidWithdrawals>
where
    C: WitnessSlice + AccountsSlice,
{
    if let Some(withdrawals) = withdrawals {
        withdrawals
            .iter()
            .enumerate()
            .try_for_each(|(position, (raw_account, _))| {
                // TODO: This parsing should happen when we first deserialise the block, and
                // not in the middle of rules validations.
                let credential = Address::from_bytes(raw_account)
                    .ok()
                    .and_then(|account| account.credential())
                    .ok_or_else(|| InvalidWithdrawals::MalformedRewardAccount {
                        bytes: raw_account.to_vec(),
                        context: TransactionField::Withdrawals,
                        position,
                    })?;

                match credential {
                    amaru_kernel::StakeCredential::ScriptHash(hash) => context
                        .require_script_witness(RequiredScript {
                            hash,
                            index: position as u32,
                            purpose: ScriptPurpose::Reward,
                            datum: MemoizedDatum::None,
                        }),
                    amaru_kernel::StakeCredential::AddrKeyhash(hash) => {
                        context.require_vkey_witness(hash)
                    }
                };

                context.withdraw_from(credential);

                Ok(())
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::TransactionField,
    };
    use amaru_kernel::{include_cbor, include_json, json, KeepRaw, MintedTransactionBody};
    use test_case::test_case;
    use tracing_json::assert_trace;

    use super::InvalidWithdrawals;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
            )
        };
    }

    #[test_case(fixture!("f861e92f12e12a744e1392a29fee5c49b987eae5e75c805f14e6ecff4ef13ff7"))]
    #[test_case(fixture!("a81147b58650b80f08986b29dad7f5efedd53ff215c17659f9dd0596e9a3d227"))]
    #[test_case(fixture!("f861e92f12e12a744e1392a29fee5c49b987eae5e75c805f14e6ecff4ef13ff7", "malformed-account") =>
        matches Err(InvalidWithdrawals::MalformedRewardAccount {  position, bytes, context })
            if  position == 0 && bytes == vec![0x00, 0x00] && matches!(context, TransactionField::Withdrawals);
        "Malformed Reward Account")]
    fn valid_withdrawal(
        (tx, expected_traces): (KeepRaw<'_, MintedTransactionBody<'_>>, Vec<json::Value>),
    ) -> Result<(), InvalidWithdrawals> {
        assert_trace(
            || {
                let mut context = AssertValidationContext::from(AssertPreparationContext {
                    utxo: Default::default(),
                });

                super::execute(&mut context, tx.withdrawals.as_deref())
            },
            expected_traces,
        )
    }
}
