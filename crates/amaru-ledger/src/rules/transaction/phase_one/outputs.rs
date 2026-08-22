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
    Address, HasScriptHash, Hash, Lovelace, MemoizedDatum, MemoizedScript, MemoizedTransactionOutput, Network,
    ProtocolParameters, ProtocolVersion, TransactionInput, Value, size::SCRIPT, utils::string::display_collection,
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_uplc::{arena::Arena, flat::decode_plutus_script};
use thiserror::Error;

use crate::{
    context::{BalanceSlice, UtxoSlice, WitnessSlice},
    rules::WithPosition,
};

mod inherent_value;

#[derive(Debug, Error)]
#[error("invalid transaction outputs: [{}]", display_collection(invalid_outputs))]
pub struct InvalidOutputs {
    pub(crate) invalid_outputs: Vec<WithPosition<InvalidOutput>>,
}

#[derive(Debug, Error)]
pub enum InvalidOutput {
    #[error("output doesn't contain enough Lovelace: minimum: {minimum_value}, given: {given_value}")]
    TooSmall { minimum_value: Lovelace, given_value: Lovelace },

    #[error("output value is too large: maximum: {maximum_size}, actual: {given_size}")]
    ValueTooLarge { maximum_size: usize, given_size: usize },

    #[error("address has the wrong network: expected: {expected}, actual: {actual}")]
    WrongNetwork { expected: Network, actual: Network },

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
    arena_pool: &ArenaPool,
    protocol_parameters: &ProtocolParameters,
    network: Network,
    outputs: Vec<MemoizedTransactionOutput>,
    supplemental_datum_policy: SupplementalDatumPolicy,
    construct_utxo: impl Fn(&mut C, u64, &Value) -> Option<TransactionInput>,
) -> Result<(), InvalidOutputs>
where
    C: WitnessSlice + UtxoSlice + BalanceSlice,
{
    let mut invalid_outputs = Vec::new();
    let arena = arena_pool.acquire();

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
            context.allow_supplemental_datum(*hash.as_ref());
        }

        if let Some(script) = output.script.as_ref() {
            validate_reference_script(script, protocol_parameters.protocol_version, &arena)
                .unwrap_or_else(|element| invalid_outputs.push(WithPosition { position, element }));
        }

        if let Some(input) = construct_utxo(context, position as u64, &output.value) {
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
        let size: usize =
            addr.attributes.iter().try_fold(0usize, |acc, attr| Ok::<_, InvalidOutput>(acc + attr.1.len()))?;

        if size > 64 {
            return Err(InvalidOutput::BootAddrAttrsTooBig { size });
        }
    }
    Ok(())
}

fn validate_network(output: &MemoizedTransactionOutput, expected: Network) -> Result<(), InvalidOutput> {
    let actual = output.address.network();

    if actual != expected {
        return Err(InvalidOutput::WrongNetwork { expected, actual });
    }

    Ok(())
}

fn validate_reference_script(
    script: &MemoizedScript,
    protocol_version: ProtocolVersion,
    arena: &Arena,
) -> Result<(), InvalidOutput> {
    let result = match script {
        MemoizedScript::PlutusV1Script(s) => decode_plutus_script(s, protocol_version, arena).map(|_| ()),
        MemoizedScript::PlutusV2Script(s) => decode_plutus_script(s, protocol_version, arena).map(|_| ()),
        MemoizedScript::PlutusV3Script(s) => decode_plutus_script(s, protocol_version, arena).map(|_| ()),
        MemoizedScript::NativeScript(_) => return Ok(()),
    };
    result.map_err(|_| InvalidOutput::MalformedReferenceScript(script.script_hash()))
}
