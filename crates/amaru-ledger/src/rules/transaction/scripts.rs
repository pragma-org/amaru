use std::collections::BTreeSet;

use amaru_kernel::{
    get_provided_scripts, HasScriptHash, MintedWitnessSet, ScriptHash, TransactionInput,
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required scripts: missing {0:?}")]
    MissingRequiredScripts(Vec<ScriptHash>),
    #[error("extraneous script witnesses: extra {0:?}")]
    ExtraneousScriptWitnesses(Vec<ScriptHash>),
}

pub fn execute<C>(
    context: &mut C,
    reference_inputs: Option<&Vec<TransactionInput>>,
    inputs: &Vec<TransactionInput>,
    witness_set: &MintedWitnessSet<'_>,
) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice,
{
    let required_scripts = context.required_scripts();

    // provided reference scripts from inputs and reference inputs only include ScriptRefs that are required by an input
    let provided_reference_scripts = [
        reference_inputs.unwrap_or(&vec![]).as_slice(),
        inputs.as_slice(),
    ]
    .concat()
    .iter()
    .filter_map(|input| {
        context
            // We assume that the reference input exists as that's validated during the inputs validation
            .lookup(input)
            .and_then(|output| match output {
                amaru_kernel::PseudoTransactionOutput::PostAlonzo(transaction_output) => {
                    transaction_output
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
                        })
                }
                amaru_kernel::PseudoTransactionOutput::Legacy(_) => None,
            })
    })
    .collect::<BTreeSet<_>>();

    let provided_scripts: BTreeSet<_> = get_provided_scripts(witness_set)
        .into_iter()
        .chain(provided_reference_scripts)
        .collect();

    let missing_scripts: Vec<ScriptHash> = required_scripts
        .difference(&provided_scripts)
        .cloned()
        .collect();

    if !missing_scripts.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing_scripts));
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

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{include_cbor, include_json, MintedTransactionBody, MintedWitnessSet};
    use test_case::test_case;

    use super::InvalidScripts;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/witness.cbor"
                )),
            )
        };
    }

    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e"); "happy path")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-scripts") =>
        matches Err(InvalidScripts::MissingRequiredScripts(..)); "missing required scripts")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-script-witness") =>
        matches Err(InvalidScripts::ExtraneousScriptWitnesses(..)); "extraneous script witness")]
    fn test_scripts(
        (mut ctx, tx, witness_set): (
            AssertValidationContext,
            MintedTransactionBody<'_>,
            MintedWitnessSet<'_>,
        ),
    ) -> Result<(), InvalidScripts> {
        super::execute(
            &mut ctx,
            tx.reference_inputs.as_deref(),
            tx.inputs.deref(),
            &witness_set,
        )
    }
}
