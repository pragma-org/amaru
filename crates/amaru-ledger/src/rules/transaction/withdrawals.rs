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

use crate::{context::WitnessSlice, rules::TransactionField};
use amaru_kernel::{Address, HasOwnership, Lovelace, RewardAccount};
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

pub(crate) fn execute(
    context: &mut impl WitnessSlice,
    withdrawals: Option<&Vec<(RewardAccount, Lovelace)>>,
) -> Result<(), InvalidWithdrawals> {
    if let Some(withdrawals) = withdrawals {
        withdrawals
            .iter()
            .enumerate()
            .try_for_each(|(position, (raw_account, _))| {
                // TODO: This parsing should happen when we first deserialise the block, and
                // not in the middle of rules validations.
                let account = Address::from_bytes(raw_account).map_err(|_| {
                    InvalidWithdrawals::MalformedRewardAccount {
                        bytes: raw_account.to_vec(),
                        context: TransactionField::Withdrawals,
                        position,
                    }
                })?;

                if let Some(credential) = account.credential() {
                    context.require_witness(credential)
                };

                Ok(())
            })?;
    }

    Ok(())
}
