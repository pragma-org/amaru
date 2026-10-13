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
    ProtocolParameters, ProtocolVersion, TransactionId, TransactionInput, size::SCRIPT,
    utils::string::display_collection,
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_uplc::{arena::Arena, flat::decode_plutus_script};
use thiserror::Error;

use crate::{
    context::{BalanceSlice, UtxoSliceMut, WitnessSlice},
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
}

/// Validates every output, including the collateral return. When the transaction is valid, its
/// regular outputs become UTxOs. When it is invalid, the collateral return does instead, at the
/// index following the regular outputs.
#[expect(clippy::too_many_arguments)]
pub(crate) fn execute<C>(
    context: &mut C,
    is_valid: bool,
    arena_pool: &ArenaPool,
    protocol_parameters: &ProtocolParameters,
    network: Network,
    transaction_id: TransactionId,
    outputs: Vec<MemoizedTransactionOutput>,
    collateral_return: Option<MemoizedTransactionOutput>,
) -> Result<(), InvalidOutputs>
where
    C: WitnessSlice + UtxoSliceMut + BalanceSlice,
{
    let mut invalid_outputs = Vec::new();
    let arena = arena_pool.acquire();
    let collateral_return_index = outputs.len() as u16;

    for (position, output) in outputs.into_iter().enumerate() {
        validate(&output, position, protocol_parameters, network, &arena, &mut invalid_outputs);

        if let MemoizedDatum::Hash(hash) = &output.datum {
            context.allow_supplemental_datum(*hash.as_ref());
        }

        context.produce_value(&output.value);

        if is_valid {
            context.produce(TransactionInput { transaction_id: *transaction_id, index: position as u16 }, output);
        }
    }

    if let Some(output) = collateral_return {
        validate(&output, collateral_return_index as usize, protocol_parameters, network, &arena, &mut invalid_outputs);

        if !is_valid {
            context
                .produce(TransactionInput { transaction_id: *transaction_id, index: collateral_return_index }, output);
        }
    }

    if !invalid_outputs.is_empty() {
        return Err(InvalidOutputs { invalid_outputs });
    }

    Ok(())
}

fn validate(
    output: &MemoizedTransactionOutput,
    position: usize,
    protocol_parameters: &ProtocolParameters,
    network: Network,
    arena: &Arena,
    invalid_outputs: &mut Vec<WithPosition<InvalidOutput>>,
) {
    let mut push = |element| invalid_outputs.push(WithPosition { position, element });

    inherent_value::execute(protocol_parameters, output).unwrap_or_else(&mut push);
    validate_network(output, network).unwrap_or_else(&mut push);
    validate_bootstrap_attributes(output).unwrap_or_else(&mut push);

    if let Some(script) = output.script.as_ref() {
        validate_reference_script(script, protocol_parameters.protocol_version, arena).unwrap_or_else(&mut push);
    }
}

fn validate_bootstrap_attributes(output: &MemoizedTransactionOutput) -> Result<(), InvalidOutput> {
    if let Address::Byron(addr) = &output.address {
        let size: usize = addr.attributes.iter().map(|attr| attr.1.len()).sum();

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

#[cfg(test)]
mod tests {
    use amaru_kernel::{Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Value};
    use test_case::test_case;

    use super::*;
    use crate::{context::DefaultValidationContext, state::volatile::VolatileFragment};

    #[test_case(true, 0; "valid transaction produces its outputs")]
    #[test_case(false, 1; "invalid transaction produces its collateral return")]
    fn produced_utxo(is_valid: bool, expected_index: u16) {
        let mut context = DefaultValidationContext::default();
        let transaction_id = TransactionId::new(Hash::new([0; 32]));

        execute(
            &mut context,
            is_valid,
            &ArenaPool::new(1, 1_024_000),
            &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
            Network::Testnet,
            transaction_id,
            vec![output()],
            Some(output()),
        )
        .expect("valid outputs");

        let produced: Vec<_> = VolatileFragment::from(context).utxo.produced.keys().copied().collect();
        assert_eq!(produced, vec![TransactionInput { transaction_id: *transaction_id, index: expected_index }]);
    }

    fn output() -> MemoizedTransactionOutput {
        let address = Address::from_bech32("addr_test1vp0ksclfnd0zjtfu70npnccut6sjex8w9k0h246xrsl089qnvvmuc")
            .expect("valid address");
        MemoizedTransactionOutput::new(false, address, Value::Coin(2_000_000), MemoizedDatum::None, None)
    }
}
