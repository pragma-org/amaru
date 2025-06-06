use std::collections::BTreeSet;

use amaru_kernel::{
    display_collection, get_provided_scripts, script_purpose_to_string, BorrowedScript,
    DatumOption, Hash, MintedWitnessSet, OriginalHash, RedeemersKey, RequiredScript, ScriptHash,
    ScriptPurpose, ScriptRefWithHash,
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
    #[error("extraneous redeemers: [{}]", .0.iter().map(|redeemer_key| format!("[{}, {}]", script_purpose_to_string(redeemer_key.tag), redeemer_key.index)).collect::<Vec<_>>().join(", "))]
    ExtraneousRedeemers(Vec<RedeemersKey>),
    #[error("missing redeemers: [{}]", .0.iter().map(|redeemer_key| format!("[{}, {}]", script_purpose_to_string(redeemer_key.tag), redeemer_key.index)).collect::<Vec<_>>().join(", "))]
    MissingRedeemers(Vec<RedeemersKey>),
}

pub fn execute<C>(context: &mut C, witness_set: &MintedWitnessSet<'_>) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice,
{
    let required_scripts = context.required_scripts();
    let required_script_hashes = required_scripts
        .iter()
        .map(ScriptHash::from)
        .collect::<BTreeSet<_>>();
    let allowed_supplemental_datum = context.allowed_supplemental_datums();

    let provided_script_refs = context.known_scripts();

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

    let provided_script_hashes: BTreeSet<ScriptHash> =
        provided_scripts.iter().map(|script| script.hash).collect();

    let missing_scripts: Vec<ScriptHash> = required_script_hashes
        .difference(&provided_script_hashes)
        .cloned()
        .collect();

    if !missing_scripts.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing_scripts));
    }

    let extra_scripts: Vec<ScriptHash> = provided_script_hashes
        .difference(&required_script_hashes)
        .copied()
        .collect();

    if !extra_scripts.is_empty() {
        return Err(InvalidScripts::ExtraneousScriptWitnesses(extra_scripts));
    }

    let required_scripts = required_scripts
        .into_iter()
        .map(|required_script| {
            if let Some(script) = provided_scripts
                .iter()
                .find(|script| script.hash == required_script.hash)
            {
                (required_script, &script.script)
            } else {
                unreachable!("required script found missing after validation");
            }
        })
        .collect::<Vec<_>>();

    let mut input_datum_hashes: BTreeSet<Hash<32>> = BTreeSet::new();
    let mut input_indices_missing_datum = Vec::new();

    required_scripts.iter().for_each(
        |(
            RequiredScript {
                index,
                datum_option,
                hash: _,
                purpose,
            },
            script,
        )| {
            if purpose == &ScriptPurpose::Spend {
                match datum_option {
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
                };
            }
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

    let mut redeemers_required = required_scripts
        .iter()
        .filter_map(|(required_script, script)| match script {
            BorrowedScript::PlutusV1Script(..)
            | BorrowedScript::PlutusV2Script(..)
            | BorrowedScript::PlutusV3Script(..) => Some(RedeemersKey::from(required_script)),
            BorrowedScript::NativeScript(_) => None,
        })
        .collect::<Vec<_>>();

    let mut extra_redeemers = Vec::new();

    if let Some(redeemers) = witness_set.redeemer.as_deref() {
        match redeemers {
            amaru_kernel::Redeemers::List(redeemers) => {
                /* It's possible that a list could have a (tag, index) tuple present more than once.
                The haskell node removes duplicates, keeping the last value present
                See (https://github.com/IntersectMBO/cardano-ledger/blob/607a7fdad352eb72041bb79f37bc1cf389432b1d/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxWits.hs#L626):
                    - The Map.fromList behavior is documented here: https://hackage.haskell.org/package/containers-0.6.6/docs/Data-Map-Strict.html#v:fromList

                This will be relevant during Phase 2 validation as well, so when that edge case inevitably pops up, refer back to this

                In this case, we don't care about the data provided in the redeemer, we only care about the presence of a needed redeemer.
                Therefore, order doesn't matter in this case.
                */
                let mut processed_keys: Vec<RedeemersKey> = Vec::new();
                redeemers.iter().for_each(|redeemer| {
                    let provided = RedeemersKey {
                        tag: redeemer.tag,
                        index: redeemer.index,
                    };

                    if !processed_keys.contains(&provided) {
                        if let Some(index) = redeemers_required
                            .iter()
                            .position(|required| required == &provided)
                        {
                            redeemers_required.remove(index);
                        } else {
                            extra_redeemers.push(provided.clone());
                        }

                        processed_keys.push(provided);
                    }
                });
            }

            // A map guarantees uniqueness of the RedeemerKey, therefore we don't need to do the same uniquness logic
            amaru_kernel::Redeemers::Map(redeemers) => {
                redeemers.iter().for_each(|(provided, _)| {
                    if let Some(index) = redeemers_required
                        .iter()
                        .position(|required| required == provided)
                    {
                        redeemers_required.remove(index);
                    } else {
                        extra_redeemers.push(provided.clone());
                    }
                });
            }
        }
    }

    if !redeemers_required.is_empty() {
        return Err(InvalidScripts::MissingRedeemers(redeemers_required));
    }

    if !extra_redeemers.is_empty() {
        return Err(InvalidScripts::ExtraneousRedeemers(extra_redeemers));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{include_cbor, include_json, MintedWitnessSet};
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
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-redeemer") =>
        matches Err(InvalidScripts::MissingRedeemers{..});
        "missing required redeemer"
    )]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-redeemer") =>
        matches Err(InvalidScripts::ExtraneousRedeemers{..});
        "extraneous redeemer"
    )]
    #[test_case(fixture!("83036e0c9851c1df44157a8407b1daa34f25549e0644f432e655bd80b0429eba"); "duplicate redeemers")]
    fn test_scripts(
        (mut ctx, witness_set): (AssertValidationContext, MintedWitnessSet<'_>),
    ) -> Result<(), InvalidScripts> {
        super::execute(&mut ctx, &witness_set)
    }
}
