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

use crate::{
    context::{UtxoSlice, WitnessSlice},
    rules::{WithPosition, format_vec},
};
use amaru_kernel::{
    HasAddress, HasNetwork, Lovelace, MemoizedDatum, MemoizedTransactionOutput,
    MintedTransactionOutput, Network, TransactionInput, protocol_parameters::ProtocolParameters,
    to_network_id,
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

pub fn execute<C>(
    context: &mut C,
    protocol_parameters: &ProtocolParameters,
    network: &Network,
    outputs: Vec<MintedTransactionOutput<'_>>,
    construct_utxo: impl Fn(u64) -> Option<TransactionInput>,
) -> Result<(), InvalidOutputs>
where
    C: WitnessSlice + UtxoSlice,
{
    let mut invalid_outputs = Vec::new();
    for (position, output) in outputs.into_iter().enumerate() {
        inherent_value::execute(protocol_parameters, &output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        validate_network(&output, network)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        match MemoizedTransactionOutput::try_from(output) {
            Ok(output) => {
                // FIXME: This line is wrong. According to the Haskell source code, we should only count
                // supplemental datums for outputs (regardless of whether transaction fails or not).
                //
                // In particular, any datum present in a collateral return does NOT count towards the
                // allowed supplemental datums.
                //
                // However, I am not fixing this now, because we have no test covering the case whatsoever.
                // At the moment, that line can actually be fully removed without making any test fail.
                if let MemoizedDatum::Hash(hash) = &output.datum {
                    context.allow_supplemental_datum(*hash);
                }

                if let Some(input) = construct_utxo(position as u64) {
                    context.produce(input, output);
                }
            }
            Err(err) => {
                let element =
                    InvalidOutput::UncategorizedError(format!("failed to convert output: {err}"));
                invalid_outputs.push(WithPosition { position, element });
            }
        }
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
        MintedTransactionBody, Network, include_cbor, protocol_parameters::ProtocolParameters,
    };
    use test_case::test_case;

    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::{WithPosition, transaction::outputs::InvalidOutput},
    };

    use super::InvalidOutputs;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                amaru_kernel::protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
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
                amaru_kernel::protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
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
        fixture!(
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2",
            ProtocolParameters {
                lovelace_per_utxo_byte: 100_000_000_000,
                ..amaru_kernel::protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone()
            }
        ) => matches Err(InvalidOutputs{invalid_outputs})
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::TooSmall { .. }
            });
        "output too small")]
    #[test_case(fixture!(
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2",
            ProtocolParameters {
                max_value_size: 1,
                ..amaru_kernel::protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone()
            }
        ) => matches Err(InvalidOutputs{invalid_outputs})
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
        super::execute(
            &mut context,
            &protocol_parameters,
            &Network::Testnet,
            tx.outputs,
            |_| None,
        )
    }
}
