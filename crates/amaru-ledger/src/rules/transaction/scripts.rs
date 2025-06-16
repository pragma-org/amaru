use std::collections::BTreeSet;

use amaru_kernel::{
    display_collection, get_provided_scripts, BorrowedScript, DatumOption, Hash, OriginalHash,
    RequiredScript, ScriptHash, ScriptPurpose, ScriptRefWithHash, WitnessSet,
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required scripts: missing [{}]", display_collection(.0))]
    MissingRequiredScripts(Vec<ScriptHash>),
    #[error("extraneous script witnesses: extra [{}]", display_collection(.0))]
    ExtraneousScriptWitnesses(Vec<ScriptHash>),
    #[error(
        "unspendable inputs; no datums. Input indices: [{}]",
        display_collection(.0)
    )]
    UnspendableInputsNoDatums(Vec<u32>),
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
}

pub fn execute<C>(context: &mut C, witness_set: &WitnessSet<'_>) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice,
{
    let provided_script_refs = context.known_scripts();

    let required_scripts = context.required_scripts();
    let required_script_hashes = required_scripts
        .iter()
        .map(ScriptHash::from)
        .collect::<BTreeSet<_>>();

    // we only consider script references required by the transaction
    let script_references = provided_script_refs
        .into_iter()
        .filter_map(|(script_hash, script_ref)| {
            if required_script_hashes.contains(&script_hash) {
                Some(ScriptRefWithHash {
                    hash: script_hash,
                    script: BorrowedScript::from(script_ref),
                })
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();

    let provided_scripts: BTreeSet<_> = get_provided_scripts(witness_set)
        .into_iter()
        .chain(script_references)
        .collect();

    let missing_scripts: Vec<ScriptHash> = required_script_hashes
        .difference(
            &provided_scripts
                .iter()
                .map(ScriptHash::from)
                .collect::<BTreeSet<_>>(),
        )
        .cloned()
        .collect();

    if !missing_scripts.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing_scripts));
    }

    let extra_scripts: Vec<ScriptHash> =
        provided_scripts
            .iter()
            .fold(Vec::new(), |mut accum, script| {
                if !required_script_hashes.contains(&script.hash) {
                    accum.push(script.hash);
                }

                accum
            });

    if !extra_scripts.is_empty() {
        return Err(InvalidScripts::ExtraneousScriptWitnesses(extra_scripts));
    }

    let required_spending_scripts = required_scripts
        .iter()
        .filter_map(|required_script| {
            if required_script.purpose == ScriptPurpose::Spend {
                if let Some(script) = provided_scripts
                    .iter()
                    .find(|script| script.hash == required_script.hash)
                {
                    return Some((required_script, &script.script));
                }
            }
            None
        })
        .collect::<Vec<_>>();

    let mut input_datum_hashes: BTreeSet<Hash<32>> = BTreeSet::new();
    let mut input_indices_missing_datum = Vec::new();

    required_spending_scripts.iter().for_each(
        |(
            RequiredScript {
                index,
                datum_option,
                hash: _,
                purpose: _,
            },
            script,
        )| match datum_option {
            None => match script {
                BorrowedScript::PlutusV1Script(..) | BorrowedScript::PlutusV2Script(..) => {
                    input_indices_missing_datum.push(*index);
                }
                BorrowedScript::NativeScript(..) | BorrowedScript::PlutusV3Script(..) => {}
            },
            Some(DatumOption::Hash(hash)) => {
                input_datum_hashes.insert(*hash);
            }
            Some(..) => {}
        },
    );

    if !input_indices_missing_datum.is_empty() {
        return Err(InvalidScripts::UnspendableInputsNoDatums(
            input_indices_missing_datum,
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

    let allowed_supplemental_datum = context.allowed_supplemental_datums();
    let supplemental_datums = witness_datum_hashes
        .difference(&input_datum_hashes)
        .cloned()
        .collect::<BTreeSet<_>>();

    let extraneous_supplemental_datums = supplemental_datums
        .difference(&allowed_supplemental_datum)
        .cloned()
        .collect::<Vec<_>>();

    if !extraneous_supplemental_datums.is_empty() {
        return Err(InvalidScripts::ExtraneousSupplementalDatums {
            provided: supplemental_datums,
            allowed: allowed_supplemental_datum,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{include_cbor, include_json, WitnessSet};
    use test_case::test_case;

    use super::InvalidScripts;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
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
                    "/witness.cbor"
                )),
            )
        };
    }

    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e"); "happy path")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "supplemental-datum-output");
        "supplemental datum output"
    )]
    #[test_case(fixture!("99cd1c8159255cf384ece25f5516fa54daaee6c5efb3f006ecf9780a0775b1dc"); "reference script in inputs")]
    #[test_case(fixture!("e974fecbf45ac386a76605e9e847a2e5d27c007fdd0be674cbad538e0c35fe01", "required-scripts"); "proposal script")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-scripts") =>
        matches Err(InvalidScripts::MissingRequiredScripts(..));
        "missing required scripts"
    )]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-script-witness") =>
        matches Err(InvalidScripts::ExtraneousScriptWitnesses(..));
        "extraneous script witness"
    )]
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
        (mut ctx, witness_set): (AssertValidationContext, WitnessSet<'_>),
    ) -> Result<(), InvalidScripts> {
        super::execute(&mut ctx, &witness_set)
    }
}
