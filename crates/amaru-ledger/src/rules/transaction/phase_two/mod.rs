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

use core::fmt;
use std::collections::BTreeMap;

use amaru_kernel::{
    ArenaPool, EraHistory, KeepRaw, MintedTransactionBody, MintedWitnessSet, OriginalHash,
    TransactionInputAdapter, TransactionPointer, cbor, network::NetworkName,
    protocol_parameters::ProtocolParameters, to_cbor,
};
use amaru_plutus::{
    script_context::{Script, TxInfo, TxInfoTranslationError, Utxos},
    to_plutus_data::{PLUTUS_V1, PLUTUS_V2, PLUTUS_V3, PlutusDataError},
};
use thiserror::Error;
use uplc_turbo::{
    binder::DeBruijn,
    constant::Constant,
    data::PlutusData,
    flat::{self, FlatDecodeError},
    machine::{ExBudget, MachineInfo, PlutusVersion},
    term::Term,
};

use crate::context::UtxoSlice;

#[derive(Debug, Error)]
pub enum PhaseTwoError {
    #[error("missing input: {0}")]
    MissingInput(TransactionInputAdapter),
    #[error("failed to translate transaction to TxInfo: {0}")]
    TransactionTranslationError(#[from] TxInfoTranslationError),
    #[error("illegal state in ScriptContext: {0}")]
    ScriptContextStateError(#[from] PlutusDataError),
    #[error("failed to deserialize script: {0}")]
    ScriptDeserializationError(cbor::decode::Error),
    #[error("failed to flat decode script: {0}")]
    FlatDecodingError(#[from] FlatDecodeError),
    #[error("missing cost models for version: {0:?}")]
    MissingCostModel(PlutusVersion),
    #[error("script evaluation failure: {0:?}")]
    UplcMachineError(UplcMachineError),
    #[error("expected scripts to fail but didn't")]
    ValidityStateError,
}

#[derive(Debug)]
pub struct UplcMachineError {
    pub plutus_version: PlutusVersion,
    pub info: MachineInfo,
    // This should be a `MachineError`, but I'm avoiding lifetime hell for now
    pub err: String,
}

#[allow(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network: &NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    pointer: TransactionPointer,
    is_valid: bool,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    transaction_witness_set: &MintedWitnessSet<'_>,
) -> Result<(), PhaseTwoError>
where
    C: UtxoSlice + fmt::Debug,
{
    if transaction_witness_set.redeemer.is_none() {
        return Ok(());
    }

    let mut resolved_inputs = transaction_body
        .inputs
        .into_iter()
        .map(|input| {
            Ok((
                input.clone(),
                context
                    .lookup(input)
                    .ok_or(PhaseTwoError::MissingInput(input.clone().into()))?
                    .clone(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()?;

    let mut resolved_reference_inptus = transaction_body
        .reference_inputs
        .as_ref()
        .map(|reference_inputs| {
            reference_inputs
                .into_iter()
                .map(|input| {
                    Ok((
                        input.clone(),
                        context
                            .lookup(input)
                            .ok_or(PhaseTwoError::MissingInput(input.clone().into()))?
                            .clone(),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()
        })
        .transpose()?
        .unwrap_or_default();

    resolved_inputs.append(&mut resolved_reference_inptus);
    let utxos = Utxos::from(resolved_inputs);

    let tx_info = TxInfo::new(
        transaction_body,
        transaction_witness_set,
        &transaction_body.original_hash(),
        &utxos,
        &pointer.slot,
        *network,
        era_history,
        protocol_parameters.protocol_version,
    )?;

    let scripts_to_execute = tx_info.to_script_contexts();

    let script_results = scripts_to_execute
        .into_iter()
        .map(|(script_context, script)| {
            let arena = arena_pool.acquire();
            let (args, cost_model, plutus_version) = match script {
                Script::Native(_) => {
                    unreachable!("cannot have a redeemer point to a native_script")
                }
                Script::PlutusV1(_) => (
                    script_context.to_script_args(PLUTUS_V1)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v1
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V1))?,
                    PlutusVersion::V1,
                ),
                Script::PlutusV2(_) => (
                    script_context.to_script_args(PLUTUS_V2)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v2
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V2))?,
                    uplc_turbo::machine::PlutusVersion::V2,
                ),
                Script::PlutusV3(_) => (
                    script_context.to_script_args(PLUTUS_V3)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v3
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V3))?,
                    uplc_turbo::machine::PlutusVersion::V3,
                ),
            };

            let script_bytes = script
                .to_bytes()
                .map_err(PhaseTwoError::ScriptDeserializationError)?;

            let mut program =
                flat::decode::<DeBruijn>(&arena, &script_bytes).map_err(PhaseTwoError::from)?;

            // TODO: we should stop using Pallas' PlutusData
            // We are using Pallas' PlutusData in `amaru-plutus` and not the `PlutusData` from `uplc`
            // so we have to do this really bad conversion (uplc uses references in plutus data, so we have to make sure everything lives long enough)
            let constants = args
                .iter()
                .map(|arg| {
                    let bytes = to_cbor(&arg);
                    #[allow(clippy::expect_used)]
                    let data = PlutusData::from_cbor(&arena, &bytes)
                        .expect("unable to decode PlutusData cbor");
                    Constant::Data(data)
                })
                .collect::<Vec<_>>();
            let arguments = constants.iter().map(Term::Constant).collect::<Vec<_>>();

            for term in arguments.iter() {
                program = program.apply(&arena, term);
            }

            let budget = script_context.budget();

            let uplc_budget = ExBudget {
                mem: budget.mem as i64,
                cpu: budget.steps as i64,
            };

            let result = program.eval_with_params(&arena, plutus_version, cost_model, uplc_budget);

            match result.term {
                Ok(term) => match term {
                    Term::Error => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                        plutus_version,
                        info: result.info,
                        err: "Error term evaluated".into(),
                    })),
                    Term::Var(_)
                    | Term::Lambda { .. }
                    | Term::Apply { .. }
                    | Term::Delay(_)
                    | Term::Force(_)
                    | Term::Case { .. }
                    | Term::Constr { .. }
                    | Term::Constant(_)
                    | Term::Builtin(_) => Ok(()),
                },
                Err(e) => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                    plutus_version,
                    info: result.info,
                    err: e.to_string(),
                })),
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
