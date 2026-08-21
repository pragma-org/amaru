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

use amaru_kernel::{Credential, Lovelace, MemoizedDatum, Network, RedeemerTag, RequiredScript, RewardAccount};
use thiserror::Error;

use crate::{
    context::{AccountsSlice, BalanceSlice, WitnessSlice},
    rules::TransactionField,
};

#[derive(Debug, Error)]
pub enum InvalidWithdrawals {
    #[error("attempted to withdraw from an account ({0:?}) that is not registered")]
    AccountNotRegistered(Credential),
    #[error(
        "attempted to withdraw a different amount than the full account balance: balance {balance} withdrawal: {withdrawal}"
    )]
    IncompleteWithdrawal { balance: u64, withdrawal: u64 },
    #[error("attempted to withdraw from an account ({0:?}) that has no drep delegation")]
    MissingAccountDRepDelegation(Credential),
    #[error(
        "network mismatch in reward account in {context:?} at position {position}: expected {expected}, received {received}"
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
            .map(|(position, (account, amount))| {
                let (credential, account_network) = (account.credential(), account.network());

                if network != account_network {
                    return Err(InvalidWithdrawals::NetworkMismatch {
                        expected: network,
                        received: account_network,
                        context: TransactionField::Withdrawals,
                        position,
                    });
                };

                let account =
                    context.lookup(&credential).ok_or(InvalidWithdrawals::AccountNotRegistered(credential))?;

                if matches!(credential, Credential::KeyHash(_)) && account.drep.is_none() {
                    return Err(InvalidWithdrawals::MissingAccountDRepDelegation(credential));
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
                    amaru_kernel::Credential::ScriptHash(hash) => context.require_script_witness(RequiredScript {
                        hash,
                        index: position as u32,
                        purpose: RedeemerTag::Reward,
                        datum: MemoizedDatum::None,
                    }),
                    amaru_kernel::Credential::KeyHash(hash) => context.require_verification_key_witness(hash),
                };

                context.consume_lovelace(amount);

                // TODO: Move state management to context
                //
                // Zero-value withdrawals are effectively no-ops. So, to save space and prevent potentially problematic state,
                // we simply don't add it to the context. This logic can, and probably should, be done *in* the context, but
                // that changes the signature of `withdraw_from`.
                if is_valid && amount > 0 {
                    context.withdraw_from(credential);
                }
            });
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use amaru_kernel::{Credential, Hash, Network, RewardAccount};

    use super::InvalidWithdrawals;
    use crate::{context::DefaultValidationContext, rules::TransactionField};

    #[test]
    fn rejects_a_reward_account_on_the_wrong_network() {
        let mut context = DefaultValidationContext::default();

        let account = RewardAccount::new(Network::Mainnet, Credential::KeyHash(Hash::new([0; 28])));

        assert!(matches!(
            super::execute(&mut context, Some(vec![(account, 1_000_000)]), Network::Testnet, true),
            Err(InvalidWithdrawals::NetworkMismatch {
                expected: Network::Testnet,
                received: Network::Mainnet,
                position: 0,
                context: TransactionField::Withdrawals,
            })
        ));
    }
}
