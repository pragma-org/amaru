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

use amaru_kernel::{Lovelace, ProtocolParameters, WitnessSet, minimum_fee};

use crate::context::{BalanceSlice, UtxoSlice};

#[derive(Debug, thiserror::Error)]
pub enum InvalidFees {
    #[error("declared fee {provided} below minimum {minimum}")]
    FeeTooSmall { provided: Lovelace, minimum: Lovelace },
}

/// Validates declared fee against the computed minimum, and then
/// performs fee/collateral collection into the context pots.
///
/// `ref_scripts_size` is the total on-wire byte size of every script attached
/// to the transaction's reference-input UTxO entries; already accumulated by
/// [`crate::rules::transaction::phase_one::inputs::execute`].
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L89>
pub(crate) fn execute<C>(
    context: &mut C,
    provided: Lovelace,
    tx_size: u64,
    witness_set: &WitnessSet,
    ref_scripts_size: u64,
    pp: &ProtocolParameters,
) -> Result<Lovelace, InvalidFees>
where
    C: UtxoSlice + BalanceSlice,
{
    let minimum = minimum_fee(tx_size, witness_set, ref_scripts_size, pp);

    if provided < minimum {
        return Err(InvalidFees::FeeTooSmall { provided, minimum });
    }

    context.produce_lovelace(provided);

    Ok(provided)
}
