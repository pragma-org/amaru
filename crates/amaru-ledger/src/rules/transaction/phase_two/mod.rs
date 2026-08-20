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

use std::{collections::BTreeMap, fmt, time::Instant};

use amaru_kernel::{
    BorrowedScript, EraHistory, GlobalParameters, HasTransactionId, PlutusVersion, ProtocolParameters, RedeemerKey,
    TransactionBody, TransactionInput, TransactionPointer, TxInfo, TxInfoTranslationError, Utxos, WitnessSet, cbor,
    to_cbor, utils::duration::elapsed_and_reset,
};
use amaru_observability::debug_span;
use amaru_plutus::{
    arena_pool::ArenaPool,
    script_context::ToScriptArgs,
    to_plutus_data::{PLUTUS_V1, PLUTUS_V2, PLUTUS_V3, PlutusDataError},
};
use amaru_uplc::{
    binder::Binder,
    constant::Constant,
    data::PlutusData,
    flat::{FlatDecodeError, decode_plutus_script},
    machine::{CostModel, ExBudget, MachineInfo},
    program::Program,
    term::Term,
};
use rayon::prelude::*;
use thiserror::Error;

use crate::context::UtxoSlice;

#[derive(Debug, Error)]
pub enum PhaseTwoError {
    #[error("missing input: {0}")]
    MissingInput(TransactionInput),
    #[error("failed to translate transaction to TxInfo: {0}")]
    TransactionTranslationError(#[from] TxInfoTranslationError),
    #[error("illegal state in ScriptContext: {0}")]
    ScriptContextStateError(#[from] PlutusDataError),
    #[error("failed to deserialize script: {0}")]
    ScriptDeserializationError(cbor::decode::Error),
    #[error("failed to flat decode script: {0}")]
    FlatDecodingError(#[from] FlatDecodeError),
    #[error("script evaluation failure: {0:?}")]
    UplcMachineError(UplcMachineError),
    #[error("expected scripts to fail but didn't")]
    ValidityStateError,
    #[error("missing cost models for version = {0:?}")]
    MissingCostModel(PlutusVersion),
}

#[derive(Debug)]
pub struct UplcMachineError {
    pub plutus_version: PlutusVersion,
    pub info: MachineInfo,
    pub redeemer: RedeemerKey,
    // TODO: Proper type with lifetime
    // This should be a `MachineError`, but I'm avoiding lifetime hell for now
    pub err: String,
    // TODO: Proper type with lifetime
    pub program: String,
}

fn encode_program<'a, V>(program: &'a Program<'a, V>) -> String
where
    V: Binder<'a>,
{
    amaru_uplc::flat::encode(program).map(hex::encode).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    global_parameters: &GlobalParameters,
    pointer: TransactionPointer,
    is_valid: bool,
    transaction_body: &TransactionBody,
    transaction_witness_set: &WitnessSet,
) -> Result<(), PhaseTwoError>
where
    C: UtxoSlice + fmt::Debug,
{
    use amaru_observability::amaru::ledger::rules::PHASE_TWO;

    if transaction_witness_set.redeemer.is_none() {
        return if is_valid { Ok(()) } else { Err(PhaseTwoError::ValidityStateError) };
    }

    let span = debug_span!(ledger::rules::PHASE_TWO);
    let _guard = span.enter();

    let mut meter = Instant::now();

    let mut resolved_inputs = transaction_body
        .inputs
        .iter()
        .map(|input| Ok((*input, context.lookup(input).ok_or(PhaseTwoError::MissingInput(*input))?.clone())))
        .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()?;

    let mut resolved_reference_inputs = transaction_body
        .reference_inputs
        .as_ref()
        .map(|reference_inputs| {
            reference_inputs
                .iter()
                .map(|input| Ok((*input, context.lookup(input).ok_or(PhaseTwoError::MissingInput(*input))?.clone())))
                .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()
        })
        .transpose()?
        .unwrap_or_default();

    resolved_inputs.append(&mut resolved_reference_inputs);
    let utxos = Utxos::from(resolved_inputs);

    let tx_info = TxInfo::new(
        transaction_body,
        transaction_witness_set,
        transaction_body.tx_id(),
        &utxos,
        &pointer.slot,
        era_history,
        global_parameters,
    )?;

    let scripts_to_execute = tx_info.to_script_contexts();

    span.record(PHASE_TWO::FIELD_SCRIPT_CONTEXT_MICROS, elapsed_and_reset(&mut meter));

    let script_results = scripts_to_execute
        .into_par_iter()
        .map(|(redeemer @ RedeemerKey { tag: purpose, index }, script_context, script)| {
            use amaru_observability::amaru::ledger::transaction::script::EXECUTE;

            let span_execute = debug_span!(
                parent: &span,
                ledger::transaction::script::EXECUTE,
                purpose = %purpose,
                index = index
            );
            let _guard = span_execute.enter();

            let mut meter = Instant::now();
            let arena = arena_pool.acquire();
            span_execute.record(EXECUTE::FIELD_ACQUIRE_ARENA_MICROS, elapsed_and_reset(&mut meter));

            // TODO: The following code screams for traits.
            let (mut program, plutus_version, args, cost_model) = match script {
                BorrowedScript::Native(_) => {
                    unreachable!("cannot have a redeemer point to a native_script")
                }
                BorrowedScript::PlutusV1(plutus_script) => {
                    let (program, plutus_version) =
                        decode_plutus_script(plutus_script, protocol_parameters.protocol_version, &arena)
                            .map_err(PhaseTwoError::from)?;
                    span_execute.record(EXECUTE::FIELD_DECODE_SCRIPT_MICROS, elapsed_and_reset(&mut meter));

                    let args = script_context.to_script_args(PLUTUS_V1)?;

                    let cost_model = protocol_parameters
                        .cost_models
                        .plutus_v1
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(plutus_version))?;

                    Ok::<_, PhaseTwoError>((program, plutus_version, args, cost_model))
                }
                BorrowedScript::PlutusV2(plutus_script) => {
                    let (program, plutus_version) =
                        decode_plutus_script(plutus_script, protocol_parameters.protocol_version, &arena)
                            .map_err(PhaseTwoError::from)?;
                    span_execute.record(EXECUTE::FIELD_DECODE_SCRIPT_MICROS, elapsed_and_reset(&mut meter));

                    let args = script_context.to_script_args(PLUTUS_V2)?;

                    let cost_model = protocol_parameters
                        .cost_models
                        .plutus_v2
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(plutus_version))?;

                    Ok::<_, PhaseTwoError>((program, plutus_version, args, cost_model))
                }
                BorrowedScript::PlutusV3(plutus_script) => {
                    let (program, plutus_version) =
                        decode_plutus_script(plutus_script, protocol_parameters.protocol_version, &arena)
                            .map_err(PhaseTwoError::from)?;
                    span_execute.record(EXECUTE::FIELD_DECODE_SCRIPT_MICROS, elapsed_and_reset(&mut meter));

                    let args = script_context.to_script_args(PLUTUS_V3)?;

                    let cost_model = protocol_parameters
                        .cost_models
                        .plutus_v3
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(plutus_version))?;

                    Ok::<_, PhaseTwoError>((program, plutus_version, args, cost_model))
                }
            }?;

            // TODO: we should stop using Pallas' PlutusData
            // We are using Pallas' PlutusData in `amaru-plutus` and not the `PlutusData` from `uplc`
            // so we have to do this really bad conversion (uplc uses references in plutus data, so we have to make sure everything lives long enough)
            let constants = args
                .iter()
                .map(|arg| {
                    let bytes = to_cbor(&arg);
                    #[allow(clippy::expect_used)]
                    let data = PlutusData::from_cbor(&arena, &bytes).expect("unable to decode PlutusData cbor");
                    Constant::Data(data)
                })
                .collect::<Vec<_>>();

            let arguments = constants.iter().map(Term::Constant).collect::<Vec<_>>();

            for term in arguments.iter() {
                program = program.apply(&arena, term);
            }
            span_execute.record(EXECUTE::FIELD_BUILD_UPLC_PROGRAM_MICROS, elapsed_and_reset(&mut meter));

            let budget = script_context.budget();
            let uplc_budget = ExBudget { mem: budget.mem as i64, cpu: budget.steps as i64 };
            let result = program.eval(
                &arena,
                CostModel::new(plutus_version, protocol_parameters.protocol_version, cost_model),
                uplc_budget,
            );
            span_execute.record(EXECUTE::FIELD_EVALUATE_UPLC_PROGRAM_MICROS, elapsed_and_reset(&mut meter));

            let uplc_machine_error = |err| {
                Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                    plutus_version,
                    info: result.info,
                    err,
                    redeemer: *redeemer,
                    program: encode_program(program),
                }))
            };

            match plutus_version {
                PlutusVersion::V1 | PlutusVersion::V2 => match result.term {
                    Ok(Term::Error) => uplc_machine_error("evaluated to error term".to_owned()),
                    Ok(_) => Ok(()),
                    Err(e) => uplc_machine_error(e.to_string()),
                },

                // According to [CIP-117](https://cips.cardano.org/cip/CIP-0117),
                // Plutus V3 scripts must evaluate to a constant term of unit
                PlutusVersion::V3 => match result.term {
                    Ok(Term::Constant(Constant::Unit)) => Ok(()),
                    Ok(_) => uplc_machine_error("evaluated to a non-unit term".to_owned()),
                    Err(e) => uplc_machine_error(e.to_string()),
                },
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ());

    if is_valid {
        script_results
    } else {
        match script_results {
            Ok(_) => Err(PhaseTwoError::ValidityStateError),
            Err(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::LazyLock};

    use amaru_kernel::{
        CostModels, MemoizedTransactionOutput, NetworkName, ProtocolParameters, Transaction, TransactionPointer,
        include_cbor,
    };
    use amaru_plutus::arena_pool::ArenaPool;
    use anyhow::Result;

    use crate::context::DefaultValidationContext;

    static ARENA_POOL: LazyLock<ArenaPool> = LazyLock::new(|| ArenaPool::new(1, 1_024_000));

    macro_rules! fixture {
        ($path:literal) => {
            include_cbor!(concat!("phase-two/mainnet/74558bb6/", $path))
        };
    }

    fn default_execute_phase_two(
        network: NetworkName,
        transaction: Transaction,
        resolved_inputs: impl Iterator<Item = MemoizedTransactionOutput>,
    ) -> Result<()> {
        let utxo = transaction
            .body
            .inputs
            .iter()
            .copied()
            .chain(transaction.body.reference_inputs.clone().map(|inputs| inputs.to_vec()).unwrap_or_default())
            .zip(resolved_inputs)
            .fold(BTreeMap::new(), |mut utxo, (input, output)| {
                utxo.insert(input, output);
                utxo
            });

        let mut context = DefaultValidationContext::default().with_utxo(utxo);

        let protocol_parameters = network.as_protocol_parameters().expect("missing network defaults");
        let protocol_parameters = ProtocolParameters {
            cost_models: CostModels {
                plutus_v2: Some(vec![
                    100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100,
                    16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32,
                    132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1,
                    1000, 42921, 4, 2, 24548, 29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1,
                    141895, 32, 83150, 32, 15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285,
                    552, 1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848,
                    228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32, 85848, 228465, 122, 0, 1, 1, 85848, 228465, 122,
                    0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4, 0, 141992, 32,
                    100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32,
                    43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
                ]),
                ..protocol_parameters.cost_models.clone()
            },
            ..protocol_parameters.clone()
        };

        super::execute(
            &mut context,
            &ARENA_POOL,
            &protocol_parameters,
            network.as_era_history().expect("missing network defaults"),
            network.as_global_parameters().expect("missing network defaults"),
            default_pointer(&transaction),
            transaction.is_expected_valid,
            &transaction.body,
            &transaction.witnesses,
        )?;

        Ok(())
    }

    fn default_pointer(transaction: &Transaction) -> TransactionPointer {
        let slot = transaction
            .body
            .validity_interval_start
            .unwrap_or_default()
            .max(transaction.body.validity_interval_end.unwrap_or_default());

        TransactionPointer { slot, transaction_index: 0 }
    }

    #[test]
    fn golden_74558bb6() -> Result<()> {
        let resolved_inputs: [MemoizedTransactionOutput; 6] = [
            fixture!("resolved-input-0.cbor"),
            fixture!("resolved-input-1.cbor"),
            fixture!("resolved-input-2.cbor"),
            fixture!("resolved-input-3.cbor"),
            fixture!("resolved-input-4.cbor"),
            fixture!("resolved-input-5.cbor"),
        ];

        default_execute_phase_two(NetworkName::Mainnet, fixture!("tx.cbor"), resolved_inputs.into_iter())
    }
}
