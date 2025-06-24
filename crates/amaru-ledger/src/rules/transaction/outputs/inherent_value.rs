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

use super::InvalidOutput;
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, HasLovelace, MintedTransactionOutput, OriginalSize,
};

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    output: &MintedTransactionOutput<'_>,
) -> Result<(), InvalidOutput> {
    // This conversion is safe with no loss of information
    let minimum_value = output.original_size() as u64 * protocol_parameters.coins_per_utxo_byte;
    let given_value = output.lovelace();

    if given_value < minimum_value {
        return Err(InvalidOutput::TooSmall {
            minimum_value,
            given_value,
        });
    }

    let max_val_size = protocol_parameters.max_val_size;
    let given_val_size = match output {
        amaru_kernel::PseudoTransactionOutput::Legacy(output) => output.amount.original_size(),
        amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => output.value.original_size(),
    };

    // This conversion is safe becuase max_val_size will never be big enough to cause a problem
    if given_val_size > max_val_size as usize {
        return Err(InvalidOutput::ValueTooLarge {
            maximum_size: max_val_size as usize,
            given_size: given_val_size,
        });
    }

    Ok(())
}
