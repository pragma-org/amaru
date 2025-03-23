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

use crate::rules::{format_vec, WithPosition};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, Lovelace, MintedTransactionOutput, TransactionOutput,
};
use thiserror::Error;

mod inherent_value;

#[derive(Debug, Error)]
#[error("invalid transaction outputs: [{}]", format_vec(invalid_outputs))]
pub struct InvalidOutputs {
    invalid_outputs: Vec<WithPosition<InvalidOutput>>,
}

#[derive(Debug, Error)]
pub enum InvalidOutput {
    #[error(
        "output doesn't contain enough Lovelace: minimum: {minimum_value}, given: {given_value}"
    )]
    TooSmall {
        minimum_value: Lovelace,
        given_value: Lovelace,
    },
}

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    outputs: Vec<MintedTransactionOutput<'_>>,
    yield_output: &mut impl FnMut(u64, TransactionOutput),
) -> Result<(), InvalidOutputs> {
    let mut invalid_outputs = Vec::new();
    for (position, output) in outputs.into_iter().enumerate() {
        inherent_value::execute(protocol_parameters, &output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        // TODO: Ensures the validation context can work from references to avoid cloning data.
        yield_output(position as u64, TransactionOutput::from(output));
    }

    if !invalid_outputs.is_empty() {
        return Err(InvalidOutputs { invalid_outputs });
    }

    Ok(())
}
