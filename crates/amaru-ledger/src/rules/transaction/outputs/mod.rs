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

use crate::rules::{TransactionRuleViolation, WithPosition};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, MintedTransactionOutput, TransactionOutput,
};

mod inherent_value;

pub fn execute<'a>(
    protocol_parameters: &ProtocolParameters,
    outputs: impl Iterator<Item = &'a MintedTransactionOutput<'a>>,
    yield_output: &mut impl FnMut(u64, TransactionOutput),
) -> Result<(), TransactionRuleViolation> {
    let mut invalid_outputs = Vec::new();
    for (position, output) in outputs.enumerate() {
        inherent_value::execute(protocol_parameters, output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        // TODO: Ensures the validation context can work from references to avoid cloning data.
        yield_output(position as u64, TransactionOutput::from(output.clone()));
    }

    if !invalid_outputs.is_empty() {
        return Err(TransactionRuleViolation::InvalidOutputs { invalid_outputs });
    }

    Ok(())
}
