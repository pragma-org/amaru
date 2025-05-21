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
    protocol_parameters::ProtocolParameters, to_network_id, HasAddress, HasNetwork, Lovelace,
    MintedTransactionOutput, Network, TransactionOutput,
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
    #[error("output value is too large: maximum: {maximum_size}, actual: {given_size}")]
    ValueTooLarge {
        maximum_size: usize,
        given_size: usize,
    },

    #[error("address has the wrong network ID: expected: {expected}, actual: {actual}")]
    WrongNetwork { expected: u8, actual: u8 },

    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    network: &Network,
    outputs: Vec<MintedTransactionOutput<'_>>,
    yield_output: &mut impl FnMut(u64, TransactionOutput),
) -> Result<(), InvalidOutputs> {
    let mut invalid_outputs = Vec::new();
    for (position, output) in outputs.into_iter().enumerate() {
        inherent_value::execute(protocol_parameters, &output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        validate_network(&output, network)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        // TODO: Ensures the validation context can work from references to avoid cloning data.
        yield_output(position as u64, TransactionOutput::from(output));
    }

    if !invalid_outputs.is_empty() {
        return Err(InvalidOutputs { invalid_outputs });
    }

    Ok(())
}

fn validate_network(
    output: &MintedTransactionOutput<'_>,
    expected_network: &Network,
) -> Result<(), InvalidOutput> {
    let address = output
        .address()
        .map_err(|e| InvalidOutput::UncategorizedError(e.to_string()))?;

    let given_network = address.has_network();

    if &given_network != expected_network {
        Err(InvalidOutput::WrongNetwork {
            expected: to_network_id(expected_network),
            actual: to_network_id(&given_network),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;

    use amaru_kernel::{
        include_cbor, protocol_parameters::ProtocolParameters, BorrowedDatumOption, HasDatum,
        MintedTransactionBody, Network, TransactionOutput,
    };
    use test_case::test_case;

    use crate::{
        context::{
            assert::{AssertPreparationContext, AssertValidationContext},
            WitnessSlice,
        },
        rules::{transaction::outputs::InvalidOutput, WithPosition},
    };

    use super::InvalidOutputs;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                ProtocolParameters::default(),
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
                ProtocolParameters::default(),
            )
        };
        ($hash:literal, $pp:expr) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                $pp,
            )
        };
    }

    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2"); "valid")]
    #[test_case(
        fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", ProtocolParameters { coins_per_utxo_byte: 100_000_000_000, ..Default::default() }) =>
        matches Err(InvalidOutputs{invalid_outputs})
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::TooSmall { .. }
            });
        "output too small")]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", ProtocolParameters { max_val_size: 1, ..Default::default() }) =>
        matches Err(InvalidOutputs{invalid_outputs})
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::ValueTooLarge {..}
            });
        "value too large"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "wrong-network-shelley") =>
        matches Err(InvalidOutputs{invalid_outputs})
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::WrongNetwork { expected: 0, actual: 1 }
            });
        "wrong network shelley"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "wrong-network-byron") =>
        matches Err(InvalidOutputs{invalid_outputs})
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::WrongNetwork { expected: 0, actual: 1 }
            });
        "wrong network byron"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "valid-byron"); "valid byron")]
    fn outputs(
        (tx, protocol_parameters): (MintedTransactionBody<'_>, ProtocolParameters),
    ) -> Result<(), InvalidOutputs> {
        let mut context = AssertValidationContext::from(AssertPreparationContext {
            utxo: BTreeMap::new(),
        });
        let yield_output = &mut |_index, output: TransactionOutput| {
            if let Some(BorrowedDatumOption::Hash(hash)) = output.has_datum() {
                context.allow_supplemental_datum(*hash);
            }
        };
        super::execute(
            &protocol_parameters,
            &Network::Testnet,
            tx.outputs,
            yield_output,
        )
    }
}
