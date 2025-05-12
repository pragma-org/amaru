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
    protocol_parameters::ProtocolParameters, to_cbor, HasLovelace, MintedTransactionOutput,
};

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    output: &MintedTransactionOutput<'_>,
) -> Result<(), InvalidOutput> {
    let coins_per_utxo_byte = protocol_parameters.coins_per_utxo_byte;
    // FIXME: do not re-serialize the output here, but rely on original bytes.
    let minimum_value = to_cbor(output).len() as u64 * coins_per_utxo_byte;

    let given_value = output.lovelace();

    if given_value < minimum_value {
        return Err(InvalidOutput::TooSmall {
            minimum_value,
            given_value,
        });
    }

    let max_val_size = protocol_parameters.max_val_size;
    // FIXME: do not re-serialize the value here, but rely on original bytes.
    let given_val_size = match output {
        amaru_kernel::PseudoTransactionOutput::Legacy(output) => to_cbor(&output.amount).len(),
        amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => to_cbor(&output.value).len(),
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
