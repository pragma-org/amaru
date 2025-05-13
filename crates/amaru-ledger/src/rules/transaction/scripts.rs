use std::collections::BTreeSet;

use amaru_kernel::{
    get_provided_scripts, HasScriptHash, MintedWitnessSet, ScriptHash, TransactionInput,
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required script witnesses: {0:?}")]
    MissingRequiredScriptWitnesses(Vec<ScriptHash>),
    #[error("extraneous script witnesses: extra {0:?}")]
    ExtraneousScriptWitnesses(Vec<ScriptHash>),
}

pub fn execute<C>(
    context: &mut C,
    reference_inputs: Option<&Vec<TransactionInput>>,
    witness_set: &MintedWitnessSet<'_>,
) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice,
{
    let required_scripts = context.required_scripts();

    // provided scripts from reference inputs only include ScriptRefs that are required by an input
    let provided_reference_scripts = reference_inputs
        .map(|reference_inputs| {
            reference_inputs
                .iter()
                .filter_map(|reference_input| {
                    context
                        // We assume that the reference input exists as that's validated during the inputs validation
                        .lookup(reference_input)
                        .and_then(|output| match output {
                            amaru_kernel::PseudoTransactionOutput::PostAlonzo(
                                transaction_output,
                            ) => transaction_output
                                .script_ref
                                .as_deref()
                                .and_then(|script_ref| {
                                    // If there is a provided ScriptRef, make sure it is required by an input
                                    let hash = script_ref.script_hash();
                                    if required_scripts.contains(&hash) {
                                        Some(hash)
                                    } else {
                                        None
                                    }
                                }),
                            amaru_kernel::PseudoTransactionOutput::Legacy(_) => None,
                        })
                })
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut provided_scripts = get_provided_scripts(witness_set);
    provided_scripts.extend(provided_reference_scripts);

    let missing_scripts: Vec<ScriptHash> = required_scripts
        .difference(&provided_scripts)
        .cloned()
        .collect();
    if !missing_scripts.is_empty() {
        return Err(InvalidScripts::MissingRequiredScriptWitnesses(
            missing_scripts,
        ));
    }

    let extra_scripts: Vec<ScriptHash> = provided_scripts
        .difference(&required_scripts)
        .cloned()
        .collect();
    if !extra_scripts.is_empty() {
        return Err(InvalidScripts::ExtraneousScriptWitnesses(extra_scripts));
    }

    Ok(())
}
