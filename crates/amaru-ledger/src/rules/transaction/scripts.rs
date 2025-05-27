use std::{collections::BTreeSet, ops::Deref};

use amaru_kernel::{
    display_collection, get_provided_scripts, script_purpose_to_string, to_redeemer_keys,
    BorrowedDatumOption, BorrowedPseudoScript, HasAddress, HasDatum, HasScriptRef, Hash,
    MintedWitnessSet, OriginalHash, RedeemersKey, ScriptHash, ScriptRefWithHash, TransactionInput,
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required scripts: missing [{}]", display_collection(.0))]
    MissingRequiredScripts(Vec<ScriptHash>),
    #[error("extraneous script witnesses: extra [{}]", display_collection(.0))]
    ExtraneousScriptWitnesses(Vec<ScriptHash>),
    #[error("unspendable inputs; no datums: [{}]",
        .0
        .iter()
        .map(|input|
            format!("{}#{}", input.transaction_id, input.index)
        )
        .collect::<Vec<_>>()
        .join(", ")
    )]
    UnspendableInputsNoDatums(Vec<TransactionInput>),
    #[error(
        "missing required datums: missing [{}] provided [{}]",
        display_collection(missing),
        display_collection(provided)
    )]
    MissingRequiredDatums {
        missing: Vec<Hash<32>>,
        provided: BTreeSet<Hash<32>>,
    },
    #[error(
        "extraneous supplemental datums: allowed: [{}] provided [{}]",
        display_collection(allowed),
        display_collection(provided)
    )]
    ExtraneousSupplementalDatums {
        allowed: BTreeSet<Hash<32>>,
        provided: BTreeSet<Hash<32>>,
    },
    #[error(
        "missing expected redeemers: missing[{}]",
        missing.iter().map(|redeemer_key| format!("({},{})", script_purpose_to_string(redeemer_key.tag), redeemer_key.index)).collect::<Vec<_>>().join(", ")
    )]
    MissingRedeemers { missing: Vec<RedeemersKey> },
}

// TODO: this can be made MUCH more efficient. Remove clones, don't iterate the same list several times, etc... Lots of low hanging fruit.
pub fn execute<C>(
    context: &mut C,
    reference_inputs: Option<&Vec<TransactionInput>>,
    inputs: &[TransactionInput],
    witness_set: &MintedWitnessSet<'_>,
) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice,
{
    let required_scripts = context.required_scripts();
    let allowed_supplemental_datums = context.allowed_supplemental_datums();

    // Now do all the lookups and collect the results
    let mut resolved_inputs = Vec::new();
    for input in inputs {
        match context.lookup(input) {
            Some(output) => resolved_inputs.push((input, output)),
            None => unreachable!(
                "found an input that doesn't exist in the utxo slice: {:?}",
                input
            ),
        }
    }

    let empty_vec = vec![];
    let mut resolved_reference_inputs = Vec::new();
    for input in reference_inputs.unwrap_or(&empty_vec) {
        match context.lookup(input) {
            Some(output) => resolved_reference_inputs.push((input, output)),
            None => unreachable!(
                "found a reference input that doesn't exist in the utxo slice: {:?}",
                input
            ),
        }
    }

    // provided reference scripts from inputs and reference inputs only include ScriptRefs that are required by an input
    let provided_reference_scripts = [
        resolved_inputs.as_slice(),
        resolved_reference_inputs.as_slice(),
    ]
    .concat()
    .iter()
    .filter_map(|(_, output)| {
        if let Some(script_ref) = output.has_script_ref() {
            if required_scripts
                .iter()
                .any(|required_script| required_script.hash == script_ref.hash)
            {
                return Some(ScriptRefWithHash {
                    hash: script_ref.hash,
                    script: script_ref.script,
                });
            }
        }

        None
    })
    .collect::<BTreeSet<_>>();

    let provided_scripts: BTreeSet<_> = get_provided_scripts(witness_set)
        .into_iter()
        .chain(provided_reference_scripts)
        .collect();

    let provided_script_hashes = provided_scripts
        .iter()
        .map(ScriptHash::from)
        .collect::<BTreeSet<_>>();

    let required_script_hashes = required_scripts
        .iter()
        .map(ScriptHash::from)
        .collect::<BTreeSet<_>>();

    let missing_scripts: Vec<ScriptHash> = required_script_hashes
        .difference(&provided_script_hashes)
        .cloned()
        .collect();

    if !missing_scripts.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing_scripts));
    }

    let extra_scripts: Vec<ScriptHash> =
        provided_scripts
            .iter()
            .fold(Vec::new(), |mut accum, script| {
                if !required_scripts
                    .iter()
                    .any(|required_script| required_script.hash == script.hash)
                {
                    accum.push(script.hash);
                }

                accum
            });

    if !extra_scripts.is_empty() {
        return Err(InvalidScripts::ExtraneousScriptWitnesses(extra_scripts));
    }

    let required_script_inputs = resolved_inputs
        .iter()
        .filter_map(|input_output| {
            input_output
                .1
                .address()
                .ok()
                .and_then(|address| match address {
                    amaru_kernel::Address::Shelley(shelley_address) => {
                        if shelley_address.payment().is_script() {
                            if let Some(script) = provided_scripts
                                .iter()
                                .find(|script| &script.hash == shelley_address.payment().as_hash())
                            {
                                return Some((input_output, script));
                            }
                        }
                        None
                    }
                    amaru_kernel::Address::Byron(_) | amaru_kernel::Address::Stake(_) => None,
                })
        })
        .collect::<Vec<_>>();

    let mut input_datum_hashes: BTreeSet<Hash<32>> = BTreeSet::new();
    let mut inputs_missing_datum: Vec<&TransactionInput> = Vec::new();

    required_script_inputs
        .into_iter()
        .for_each(|((input, output), script)| match output.has_datum() {
            None => match script.script {
                BorrowedPseudoScript::PlutusV1Script(..)
                | BorrowedPseudoScript::PlutusV2Script(..) => {
                    inputs_missing_datum.push(input);
                }
                BorrowedPseudoScript::NativeScript(..)
                | BorrowedPseudoScript::PlutusV3Script(..) => {}
            },
            Some(BorrowedDatumOption::Hash(hash)) => {
                input_datum_hashes.insert(*hash);
            }
            Some(..) => {}
        });

    if !inputs_missing_datum.is_empty() {
        return Err(InvalidScripts::UnspendableInputsNoDatums(
            inputs_missing_datum
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
        ));
    }

    let witness_datum_hashes: BTreeSet<Hash<32>> = witness_set
        .plutus_data
        .as_deref()
        .map(|datums| {
            datums
                .iter()
                .map(|datum| datum.original_hash())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let unmatched_datums = input_datum_hashes
        .difference(&witness_datum_hashes)
        .cloned()
        .collect::<Vec<_>>();

    if !unmatched_datums.is_empty() {
        return Err(InvalidScripts::MissingRequiredDatums {
            missing: unmatched_datums,
            provided: input_datum_hashes,
        });
    }

    let supplemental_datums = witness_datum_hashes
        .difference(&input_datum_hashes)
        .cloned()
        .collect::<BTreeSet<_>>();

    let extraneous_supplemental_datums = supplemental_datums
        .difference(&allowed_supplemental_datums)
        .cloned()
        .collect::<Vec<_>>();

    if !extraneous_supplemental_datums.is_empty() {
        return Err(InvalidScripts::ExtraneousSupplementalDatums {
            provided: supplemental_datums,
            allowed: allowed_supplemental_datums,
        });
    }

    // TODO: refactor all the required redeemers logic. This is really gross...

    let redeemers_needed = required_scripts
        .iter()
        .filter(|required_script| {
            provided_scripts
                .iter()
                .find(|provided_script| required_script.hash == provided_script.hash)
                .and_then(|provided_script| match provided_script.script {
                    BorrowedPseudoScript::NativeScript(_) => None,
                    BorrowedPseudoScript::PlutusV1Script(..)
                    | BorrowedPseudoScript::PlutusV2Script(..)
                    | BorrowedPseudoScript::PlutusV3Script(..) => Some(true),
                })
                .unwrap_or_default()
        })
        .map(RedeemersKey::from)
        .collect::<Vec<_>>();

    let redeemers_provided = witness_set
        .redeemer
        .as_ref()
        .map(|redeemer| to_redeemer_keys(redeemer.deref()))
        .unwrap_or_default();

    let missing_redeemers = redeemers_needed
        .iter()
        .filter(|redeemer| !redeemers_provided.contains(redeemer))
        .cloned()
        .collect::<Vec<_>>();

    if !missing_redeemers.is_empty() {
        return Err(InvalidScripts::MissingRedeemers {
            missing: missing_redeemers,
        });
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
        matches Err(InvalidScripts::MissingRequiredScripts(..));
        "missing required scripts"
    )]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-script-witness") =>
        matches Err(InvalidScripts::ExtraneousScriptWitnesses(..));
        "extraneous script witness"
    )]
    #[test_case(fixture!("99cd1c8159255cf384ece25f5516fa54daaee6c5efb3f006ecf9780a0775b1dc"); "reference script in inputs")]
    #[test_case(fixture!("e974fecbf45ac386a76605e9e847a2e5d27c007fdd0be674cbad538e0c35fe01", "required-scripts"); "proposal script")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "unspendable-input") =>
        matches Err(InvalidScripts::UnspendableInputsNoDatums(..));
        "unspendable input"
    )]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-datum") =>
        matches Err(InvalidScripts::MissingRequiredDatums{..});
        "missing required datum"
    )]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-supplemental-datum") =>
        matches Err(InvalidScripts::ExtraneousSupplementalDatums{..});
        "extraneous supplemental datum"
    )]
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
