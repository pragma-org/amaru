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
    context::{AccountsSliceMut, BalanceSlice, WitnessSlice},
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

/// Requires the witness of every withdrawal, which must drain a registered account in full. That account is then
/// emptied.
pub(crate) fn when_valid<C>(
    context: &mut C,
    withdrawals: Option<Vec<(RewardAccount, Lovelace)>>,
    network: Network,
) -> Result<(), InvalidWithdrawals>
where
    C: WitnessSlice + AccountsSliceMut + BalanceSlice,
{
    for (position, (credential, amount)) in sorted_by_credential(withdrawals, network)?.into_iter().enumerate() {
        let account = context.lookup(&credential).ok_or(InvalidWithdrawals::AccountNotRegistered(credential))?;

        if matches!(credential, Credential::KeyHash(_)) && account.drep.is_none() {
            return Err(InvalidWithdrawals::MissingAccountDRepDelegation(credential));
        }

        if amount != account.rewards {
            return Err(InvalidWithdrawals::IncompleteWithdrawal { balance: account.rewards, withdrawal: amount });
        }

        require_witness_and_consume(context, position, credential, amount);

        // TODO: Move state management to context
        //
        // Zero-value withdrawals are effectively no-ops. So, to save space and prevent potentially problematic state,
        // we simply don't add it to the context. This logic can, and probably should, be done *in* the context, but
        // that changes the signature of `withdraw_from`.
        if amount > 0 {
            context.withdraw_from(credential);
        }
    }

    Ok(())
}

/// Requires the witness of every withdrawal, without checking or modifying account state.
pub(crate) fn when_invalid<C>(
    context: &mut C,
    withdrawals: Option<Vec<(RewardAccount, Lovelace)>>,
    network: Network,
) -> Result<(), InvalidWithdrawals>
where
    C: WitnessSlice + BalanceSlice,
{
    for (position, (credential, amount)) in sorted_by_credential(withdrawals, network)?.into_iter().enumerate() {
        require_witness_and_consume(context, position, credential, amount);
    }

    Ok(())
}

/// Checks the network of every reward account, in transaction order. Returns the withdrawals sorted by stake
/// credential, the order that reward redeemer indexes refer to.
fn sorted_by_credential(
    withdrawals: Option<Vec<(RewardAccount, Lovelace)>>,
    network: Network,
) -> Result<BTreeMap<Credential, Lovelace>, InvalidWithdrawals> {
    withdrawals
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(position, (account, amount))| {
            let account_network = account.network();
            if network != account_network {
                return Err(InvalidWithdrawals::NetworkMismatch {
                    expected: network,
                    received: account_network,
                    context: TransactionField::Withdrawals,
                    position,
                });
            };

            Ok((account.credential(), amount))
        })
        .collect()
}

fn require_witness_and_consume<C>(context: &mut C, position: usize, credential: Credential, amount: Lovelace)
where
    C: WitnessSlice + BalanceSlice,
{
    match credential {
        Credential::ScriptHash(hash) => context.require_script_witness(RequiredScript {
            hash,
            index: position as u32,
            purpose: RedeemerTag::Reward,
            datum: MemoizedDatum::None,
        }),
        Credential::KeyHash(hash) => context.require_verification_key_witness(hash),
    };

    context.consume_lovelace(amount);
}

#[cfg(test)]
mod test {
    use amaru_kernel::{Credential, Hash, Network, RewardAccount};
    use test_case::test_case;

    use super::InvalidWithdrawals;
    use crate::{
        context::{BalanceSlice, DefaultValidationContext},
        rules::TransactionField,
    };

    #[test]
    fn rejects_a_reward_account_on_the_wrong_network() {
        let mut context = DefaultValidationContext::default();

        let account = RewardAccount::new(Network::Mainnet, Credential::KeyHash(Hash::new([0; 28])));

        assert!(matches!(
            super::when_valid(&mut context, Some(vec![(account, 1_000_000)]), Network::Testnet),
            Err(InvalidWithdrawals::NetworkMismatch {
                expected: Network::Testnet,
                received: Network::Mainnet,
                position: 0,
                context: TransactionField::Withdrawals,
            })
        ));
    }

    #[test_case(true => matches Err(InvalidWithdrawals::AccountNotRegistered(_)); "valid transaction rejects it")]
    #[test_case(false => matches Ok(1_000_000); "invalid transaction consumes it")]
    fn withdrawal_from_an_unregistered_account(is_valid: bool) -> Result<i128, InvalidWithdrawals> {
        let mut context = DefaultValidationContext::default();

        let account = RewardAccount::new(Network::Testnet, Credential::KeyHash(Hash::new([0; 28])));

        let withdrawals = Some(vec![(account, 1_000_000)]);
        if is_valid {
            super::when_valid(&mut context, withdrawals, Network::Testnet)?;
        } else {
            super::when_invalid(&mut context, withdrawals, Network::Testnet)?;
        }

        Ok(context.balance().coin())
    }
}
