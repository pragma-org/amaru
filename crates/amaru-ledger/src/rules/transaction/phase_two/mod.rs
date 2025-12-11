use core::fmt;
use std::collections::BTreeMap;

use amaru_kernel::{
    ArenaPool, EraHistory, KeepRaw, MintedTransactionBody, MintedWitnessSet, OriginalHash,
    TransactionPointer, network::NetworkName, protocol_parameters::ProtocolParameters, to_cbor,
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
    flat,
    machine::{ExBudget, MachineInfo, PlutusVersion},
    term::Term,
};

use crate::context::UtxoSlice;

#[derive(Debug, Error)]
pub enum PhaseTwoError {
    #[error("failed to translate transaction to TxInfo: {0}")]
    TransactionTranslationError(#[from] TxInfoTranslationError),
    #[error("illegal state in ScriptContext: {0}")]
    ScriptContextStateError(#[from] PlutusDataError),
    // TODO: convert from MachineError to PhaseTwoError
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
        .map(|input| (input.clone(), context.lookup(input).unwrap().clone()))
        .collect::<BTreeMap<_, _>>();

    let mut resolved_reference_inptus = transaction_body
        .reference_inputs
        .as_ref()
        .map(|reference_inputs| {
            reference_inputs
                .into_iter()
                .map(|input| (input.clone(), context.lookup(input).unwrap().clone()))
                .collect::<BTreeMap<_, _>>()
        })
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
                    protocol_parameters.cost_models.plutus_v1.clone().unwrap(),
                    uplc_turbo::machine::PlutusVersion::V1,
                ),
                Script::PlutusV2(_) => (
                    script_context.to_script_args(PLUTUS_V2)?,
                    protocol_parameters.cost_models.plutus_v2.clone().unwrap(),
                    uplc_turbo::machine::PlutusVersion::V2,
                ),
                Script::PlutusV3(_) => (
                    script_context.to_script_args(PLUTUS_V3)?,
                    protocol_parameters.cost_models.plutus_v3.clone().unwrap(),
                    uplc_turbo::machine::PlutusVersion::V3,
                ),
            };

            let mut program =
                flat::decode::<DeBruijn>(&arena, &script.to_bytes()).expect("Failed to decode");

            // We are using Pallas' PlutusData in `amaru-plutus` and not the `PlutusData` from `uplc`
            // so we have to do this really bad conversion (uplc uses references in plutus data, so we have ot make sure everything lives long enough)
            let constants = args
                .iter()
                .map(|arg| {
                    let bytes = to_cbor(&arg);
                    let data = PlutusData::from_cbor(&arena, &bytes).unwrap();
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

            let result = program.eval_with_params(&arena, plutus_version, &cost_model, uplc_budget);

            match result.term {
                Ok(term) => match term {
                    Term::Error => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                        plutus_version,
                        info: result.info,
                        err: "Error term evaluated".into(),
                    })),
                    _ => Ok(()),
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
