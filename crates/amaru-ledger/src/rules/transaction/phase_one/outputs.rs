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

use amaru_kernel::{
    AddrAttrProperty, Address, AddressPayload, AsIndex, HasNetwork, HasScriptHash, Hash, Lovelace, MemoizedDatum,
    MemoizedScript, MemoizedTransactionOutput, Network, ProtocolParameters, ProtocolVersion, TransactionInput, cbor,
    from_cbor, size::SCRIPT, utils::string::display_collection,
};
use amaru_uplc::{arena::Arena, machine::PlutusVersion};
use thiserror::Error;

use crate::{
    context::{UtxoSlice, WitnessSlice},
    rules::{WithPosition, transaction::phase_one::scripts::validate_plutus_script},
};

mod inherent_value;

#[derive(Debug, Error)]
#[error("invalid transaction outputs: [{}]", display_collection(invalid_outputs))]
pub struct InvalidOutputs {
    invalid_outputs: Vec<WithPosition<InvalidOutput>>,
}

#[derive(Debug, Error)]
pub enum InvalidOutput {
    #[error("output doesn't contain enough Lovelace: minimum: {minimum_value}, given: {given_value}")]
    TooSmall { minimum_value: Lovelace, given_value: Lovelace },
    #[error("output value is too large: maximum: {maximum_size}, actual: {given_size}")]
    ValueTooLarge { maximum_size: usize, given_size: usize },

    #[error("address has the wrong network ID: expected: {expected}, actual: {actual}")]
    WrongNetwork { expected: u8, actual: u8 },

    #[error("malformed reference script: {0}")]
    MalformedReferenceScript(Hash<SCRIPT>),

    #[error("bootstrap address attributes too big: {size} bytes, max 64")]
    BootAddrAttrsTooBig { size: usize },

    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

/// Enum that is used to determine whether or not to allow a datum as supplemental in the context.
/// In the case of a collateral return output, datums should not be allowed as supplemental.
pub enum SupplementalDatumPolicy {
    Allow,
    Disallow,
}

pub fn execute<C>(
    context: &mut C,
    protocol_parameters: &ProtocolParameters,
    network: Network,
    outputs: Vec<MemoizedTransactionOutput>,
    supplemental_datum_policy: SupplementalDatumPolicy,
    construct_utxo: impl Fn(u64) -> Option<TransactionInput>,
) -> Result<(), InvalidOutputs>
where
    C: WitnessSlice + UtxoSlice,
{
    let mut invalid_outputs = Vec::new();
    // TODO: we should not be allocating a new arena here, instead using a shared pool, such as the one we use for phase 2 validation.
    let mut arena = Arena::new();

    for (position, output) in outputs.into_iter().enumerate() {
        inherent_value::execute(protocol_parameters, &output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        validate_network(&output, network)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        validate_bootstrap_attributes(&output)
            .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));

        if matches!(supplemental_datum_policy, SupplementalDatumPolicy::Allow)
            && let MemoizedDatum::Hash(hash) = &output.datum
        {
            context.allow_supplemental_datum(*hash);
        }

        if let Some(script) = output.script.as_ref() {
            validate_reference_script(script, protocol_parameters.protocol_version, &mut arena)
                .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));
        }

        if let Some(input) = construct_utxo(position as u64) {
            context.produce(input, output);
        }
    }

    if !invalid_outputs.is_empty() {
        return Err(InvalidOutputs { invalid_outputs });
    }

    Ok(())
}

fn validate_bootstrap_attributes(output: &MemoizedTransactionOutput) -> Result<(), InvalidOutput> {
    if let Address::Byron(addr) = &output.address {
        // This logic assumes the address (and thus the payload) has already been checked
        let Some(payload) = from_cbor::<AddressPayload>(&addr.payload.0) else {
            return Ok(());
        };

        // This differs from the Haskell logic:
        // bootstrapAddressAttrsSize (BootstrapAddress addr) =
        //  maybe 0 payloadLen derivationPath + Byron.unknownAttributesLength attrs
        //  where
        //    payloadLen = BS.length . Byron.getHDAddressPayload
        //
        // In Haskell, anything other than keys 1 (derivation path) and 2 (network magic)
        // goes into attrRemain and is counted by unknownAttributesLength. The Pallas decoder differs in two ways:
        //   1. Keys 3+ are rejected outright (decoder error), so we never see them here.
        //   2. Key 0 (AddrDistr) is decoded as a known variant, but its wire-format
        //      handling may not match Haskell's bytestring-wrapped convention.
        //
        // These points make our check here incomplete relative to Haskell.
        let size: usize = payload.attributes.iter().try_fold(0usize, |acc, attr| {
            let n = match attr {
                AddrAttrProperty::DerivationPath(bytes) => bytes.len(),
                AddrAttrProperty::AddrDistr(distr) => cbor::to_vec(distr)
                    .map(|v| v.len())
                    .map_err(|e| InvalidOutput::UncategorizedError(format!("AddrDistr re-encoding failed: {e}")))?,
                AddrAttrProperty::NetworkTag(_) => 0,
            };
            Ok::<_, InvalidOutput>(acc + n)
        })?;

        if size > 64 {
            return Err(InvalidOutput::BootAddrAttrsTooBig { size });
        }
    }
    Ok(())
}

fn validate_network(output: &MemoizedTransactionOutput, expected_network: Network) -> Result<(), InvalidOutput> {
    let given_network = output.address.has_network();

    if given_network != expected_network {
        Err(InvalidOutput::WrongNetwork { expected: expected_network.as_index(), actual: given_network.as_index() })
    } else {
        Ok(())
    }
}

fn validate_reference_script(
    script: &MemoizedScript,
    protocol_version: ProtocolVersion,
    arena: &mut Arena,
) -> Result<(), InvalidOutput> {
    let result = match script {
        MemoizedScript::PlutusV1Script(s) => validate_plutus_script(s, PlutusVersion::V1, protocol_version, arena),
        MemoizedScript::PlutusV2Script(s) => validate_plutus_script(s, PlutusVersion::V2, protocol_version, arena),
        MemoizedScript::PlutusV3Script(s) => validate_plutus_script(s, PlutusVersion::V3, protocol_version, arena),
        MemoizedScript::NativeScript(_) => return Ok(()),
    };
    result.map_err(|_| InvalidOutput::MalformedReferenceScript(script.script_hash()))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use amaru_kernel::{Hash, Network, ProtocolParameters, TransactionBody, include_cbor, size::DATUM};
    use test_case::test_case;

    use super::{InvalidOutput, InvalidOutputs, SupplementalDatumPolicy};
    use crate::{
        context::{
            WitnessSlice,
            assert::{AssertPreparationContext, AssertValidationContext},
        },
        rules::WithPosition,
    };

    type Outcome = (Result<(), InvalidOutputs>, BTreeSet<Hash<DATUM>>);

    macro_rules! fixture {
        ($hash:literal) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/tx.cbor")),
                amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $pp:expr) => {
            (include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")), $pp)
        };
    }

    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2")
        => matches (Ok(()), _);
        "valid")]
    #[test_case(
        fixture!(
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2",
            ProtocolParameters {
                lovelace_per_utxo_byte: 100_000_000_000,
                ..amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()
            }
        ) => matches (Err(InvalidOutputs{invalid_outputs}), _)
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::TooSmall { .. }
            });
        "output too small")]
    #[test_case(fixture!(
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2",
            ProtocolParameters {
                max_value_size: 1,
                ..amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()
            }
        ) => matches (Err(InvalidOutputs{invalid_outputs}), _)
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::ValueTooLarge {..}
            });
        "value too large"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "wrong-network-shelley")
        => matches (Err(InvalidOutputs{invalid_outputs}), _)
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::WrongNetwork { expected: 0, actual: 1 }
            });
        "wrong network shelley"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "wrong-network-byron")
        => matches (Err(InvalidOutputs{invalid_outputs}), _)
            if matches!(invalid_outputs[0], WithPosition {
                position: 0,
                element: InvalidOutput::WrongNetwork { expected: 0, actual: 1 }
            });
        "wrong network byron"
    )]
    #[test_case(fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2", "valid-byron")
        => matches (Ok(()), _);
        "valid byron")]
    #[test_case(fixture!("578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6")
        => matches (Ok(()), datums) if !datums.is_empty();
        "output with datum hash contributes supplemental datum"
    )]
    fn outputs((tx, protocol_parameters): (TransactionBody, ProtocolParameters)) -> Outcome {
        let mut context = AssertValidationContext::from(AssertPreparationContext { utxo: BTreeMap::new() });
        let result = super::execute(
            &mut context,
            &protocol_parameters,
            Network::Testnet,
            tx.outputs,
            SupplementalDatumPolicy::Allow,
            |_| None,
        );
        (result, context.allowed_supplemental_datums())
    }

    #[test_case(fixture!(
        "578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6",
        "collateral-return-datum-hash"
    ) => matches (Ok(()), datums) if datums.is_empty();
        "collateral return with datum hash does not contribute supplemental datum"
    )]
    fn collateral_return((tx, protocol_parameters): (TransactionBody, ProtocolParameters)) -> Outcome {
        let mut context = AssertValidationContext::from(AssertPreparationContext { utxo: BTreeMap::new() });
        let outputs = tx.collateral_return.map(|output| vec![output]).expect("fixture must have a collateral_return");
        let result = super::execute(
            &mut context,
            &protocol_parameters,
            Network::Testnet,
            outputs,
            SupplementalDatumPolicy::Disallow,
            |_| None,
        );
        (result, context.allowed_supplemental_datums())
    }
}
