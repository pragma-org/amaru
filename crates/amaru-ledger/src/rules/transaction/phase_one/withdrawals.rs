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

use std::collections::BTreeMap;

use amaru_kernel::{
    Lovelace, MemoizedDatum, Network, RedeemerTag, RequiredScript, RewardAccount, StakeCredential, parse_reward_account,
};
use thiserror::Error;

use crate::{
    context::{AccountsSlice, BalanceSlice, WitnessSlice},
    rules::TransactionField,
};

#[derive(Debug, Error)]
pub enum InvalidWithdrawals {
    #[error("unexpected bytes instead of reward account in {context:?} at position {position}")]
    MalformedRewardAccount { bytes: Vec<u8>, context: TransactionField, position: usize },
    #[error("attempted to withdraw from an account ({0:?}) that is not registered")]
    AccountNotRegistered(StakeCredential),
    #[error(
        "attempted to withdraw a different amount than the full account balance: balance {balance} withdrawal: {withdrawal}"
    )]
    IncompleteWithdrawal { balance: u64, withdrawal: u64 },
    #[error("attempted to withdraw from an account ({0:?}) that either doesn't exist or has no drep delegation")]
    MissingAccountDRepDelegation(StakeCredential),
    #[error(
        "network mismatch in reward account in {context:?} at position {position}: expected {expected:?}, received {received:?}"
    )]
    NetworkMismatch { expected: Network, received: Network, context: TransactionField, position: usize },
}

pub(crate) fn execute<C>(
    context: &mut C,
    withdrawals: Option<Vec<(RewardAccount, Lovelace)>>,
    network: Network,
    is_valid: bool,
) -> Result<(), InvalidWithdrawals>
where
    C: WitnessSlice + AccountsSlice + BalanceSlice,
{
    if let Some(withdrawals) = withdrawals {
        withdrawals
            .into_iter()
            .enumerate()
            .map(|(position, (bytes, amount))| {
                let (credential, account_network) =
                    parse_reward_account(&bytes).ok_or_else(|| InvalidWithdrawals::MalformedRewardAccount {
                        bytes: bytes.to_vec(),
                        context: TransactionField::Withdrawals,
                        position,
                    })?;

                if network != account_network {
                    return Err(InvalidWithdrawals::NetworkMismatch {
                        expected: network,
                        received: account_network,
                        context: TransactionField::Withdrawals,
                        position,
                    });
                };

                let account =
                    context.lookup(&credential).ok_or(InvalidWithdrawals::AccountNotRegistered(credential.clone()))?;

                if account.drep.is_none() {
                    return Err(InvalidWithdrawals::MissingAccountDRepDelegation(credential.clone()));
                }

                if amount != account.rewards {
                    return Err(InvalidWithdrawals::IncompleteWithdrawal {
                        balance: account.rewards,
                        withdrawal: amount,
                    });
                }

                Ok((credential, amount))
            })
            // NOTE: Force withdrawals to be sorted by stake credentials
            .collect::<Result<BTreeMap<_, _>, _>>()?
            .into_iter()
            .enumerate()
            .for_each(|(position, (credential, amount))| {
                match credential {
                    amaru_kernel::StakeCredential::ScriptHash(hash) => context.require_script_witness(RequiredScript {
                        hash,
                        index: position as u32,
                        purpose: RedeemerTag::Reward,
                        datum: MemoizedDatum::None,
                    }),
                    amaru_kernel::StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
                };

                context.consume_lovelace(amount);

                if is_valid {
                    context.withdraw_from(credential);
                }
            });
    }

    Ok(())
}
