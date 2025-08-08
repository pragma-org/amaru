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

use crate::context::{UtxoSlice, WitnessSlice};
use amaru_kernel::{
    display_collection, get_provided_scripts, script_purpose_to_string, DatumHash, HasRedeemers,
    MemoizedDatum, MintedWitnessSet, OriginalHash, RedeemerKey, RequiredScript, ScriptHash,
    ScriptKind, ScriptPurpose,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Deref,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required scripts: missing [{}]", display_collection(.0))]
    MissingRequiredScripts(BTreeSet<ScriptHash>),
    #[error("extraneous script witnesses: extra [{}]", display_collection(.0))]
    ExtraneousScriptWitnesses(BTreeSet<ScriptHash>),
    #[error(
        "unspendable inputs at position(s) [{}]: no datums",
        display_collection(.0)
    )]
    UnspendableInputsNoDatums(BTreeSet<u32>),
    #[error(
        "missing required datums: missing [{}] provided [{}]",
        display_collection(missing),
        display_collection(provided)
    )]
    MissingRequiredDatums {
        missing: BTreeSet<DatumHash>,
        provided: BTreeSet<DatumHash>,
    },
    #[error(
        "extraneous supplemental datums: supplemental: [{}], extraneous: [{}]",
        display_collection(supplemental),
        display_collection(extraneous)
    )]
    ExtraneousSupplementalDatums {
        supplemental: BTreeSet<DatumHash>,
        extraneous: BTreeSet<DatumHash>,
    },
    #[error("extraneous redeemers: [{}]", .0.iter().map(|redeemer_key| format!("[{}, {}]", script_purpose_to_string(redeemer_key.tag), redeemer_key.index)).collect::<Vec<_>>().join(", "))]
    ExtraneousRedeemers(Vec<RedeemerKey>),
    #[error("missing redeemers: [{}]", .0.iter().map(|redeemer_key| format!("[{}, {}]", script_purpose_to_string(redeemer_key.tag), redeemer_key.index)).collect::<Vec<_>>().join(", "))]
    MissingRedeemers(Vec<RedeemerKey>),
}

// TODO: Split this whole function into smaller functions to make it more graspable.
pub fn execute<C>(context: &mut C, witness_set: &MintedWitnessSet<'_>) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice + fmt::Debug,
{
    let required_scripts = context.required_scripts();

    let required_script_hashes: BTreeSet<&ScriptHash> = required_scripts
        .iter()
        .map(|RequiredScript { hash, .. }| hash)
        .collect();

    let provided_scripts = collect_provided_scripts(context, &required_script_hashes, witness_set);

    let required_scripts =
        fail_on_script_symmetric_differences(required_scripts, &provided_scripts)?;

    let (mut required_redeemers, required_datums) = partition_scripts(required_scripts)?;

    let witnessed_datums = datum_hashes(witness_set);

    fail_on_supplemental_datums(context, &required_datums, &witnessed_datums)?;

    fail_on_unmatched_datums(context, &required_datums, witnessed_datums)?;

    let mut extra_redeemers = Vec::new();

    if let Some(provided_redemeers) = witness_set.redeemer.as_deref().map(HasRedeemers::redeemers) {
        provided_redemeers.keys().for_each(|provided| {
            if let Some(index) = required_redeemers
                .iter()
                .position(|required| required == provided.deref())
            {
                required_redeemers.remove(index);
            } else {
                extra_redeemers.push(provided.deref().clone());
            }
        })
    }

    if !required_redeemers.is_empty() {
        return Err(InvalidScripts::MissingRedeemers(required_redeemers));
    }

    if !extra_redeemers.is_empty() {
        return Err(InvalidScripts::ExtraneousRedeemers(extra_redeemers));
    }

    Ok(())
}

/// Split all required scripts information into two sub-partitions:
///
/// 1. The (ordered) list of redeemer keys (purpose and index) which needs to be executed.
///
/// 2. The set of datum hash digests for which a preimage is needed.
///
/// The function fails if there's any input with missing mandatory datum (i.e. Plutus V1 or V2
/// script-locked inputs without datum; those are simply "forever" unspendable).
fn partition_scripts(
    required_scripts: Vec<(RequiredScript, &ScriptKind)>,
) -> Result<(Vec<RedeemerKey>, BTreeSet<DatumHash>), InvalidScripts> {
    let mut required_redeemers = Vec::new();
    let mut required_datums = BTreeSet::new();
    let mut missing_datums = BTreeSet::new();

    required_scripts
        .iter()
        .for_each(|(required_script, script)| {
            let RequiredScript {
                index,
                datum,
                hash: _,
                purpose,
            } = required_script;

            let mut require_redeemer =
                || required_redeemers.push(RedeemerKey::from(required_script));

            let mut unspendable_without_datum = || {
                if purpose == &ScriptPurpose::Spend && matches!(datum, MemoizedDatum::None) {
                    missing_datums.insert(*index);
                }
            };

            let mut require_datum_preimage = || match datum {
                MemoizedDatum::Hash(hash) => {
                    required_datums.insert(*hash);
                }
                MemoizedDatum::Inline(..) | MemoizedDatum::None => {}
            };

            match script {
                // NOTE: One may very well send some funds to a native script, and attach a
                // datum hash to it. In which case, the datum has no effect and is simply
                // ignored.
                ScriptKind::Native => {}

                ScriptKind::PlutusV1 => {
                    require_redeemer();
                    unspendable_without_datum();
                    require_datum_preimage();
                }

                ScriptKind::PlutusV2 => {
                    require_redeemer();
                    unspendable_without_datum();
                    require_datum_preimage();
                }

                ScriptKind::PlutusV3 => {
                    require_redeemer();
                    require_datum_preimage();
                }
            };
        });

    fail_on_missing_datums(missing_datums)?;

    Ok((required_redeemers, required_datums))
}

// TODO: Should live in Pallas.
/// Collect all datum hash digests found in the witness set.
fn datum_hashes(witness_set: &MintedWitnessSet<'_>) -> BTreeSet<DatumHash> {
    witness_set
        .plutus_data
        .as_deref()
        .map(|datums| {
            datums
                .iter()
                .map(|datum| datum.original_hash())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default()
}

fn collect_provided_scripts<'a, C>(
    context: &'a mut C,
    required: &BTreeSet<&ScriptHash>,
    witness_set: &'a MintedWitnessSet<'_>,
) -> BTreeMap<ScriptHash, ScriptKind>
where
    C: WitnessSlice,
{
    let referenced = context
        .known_scripts()
        .into_iter()
        // We only consider script references required by the transaction
        .filter_map(|(script_hash, script_ref)| {
            if required.contains(&script_hash) {
                Some((script_hash, ScriptKind::from(script_ref)))
            } else {
                None
            }
        });

    get_provided_scripts(witness_set)
        .into_iter()
        .chain(referenced)
        .collect()
}

/// Ensures that the required and provided scripts match exactly (i.e. check that they're included
/// in each other).
fn fail_on_script_symmetric_differences(
    required: BTreeSet<RequiredScript>,
    provided: &BTreeMap<ScriptHash, ScriptKind>,
) -> Result<Vec<(RequiredScript, &ScriptKind)>, InvalidScripts> {
    let mut missing = BTreeSet::new();
    let mut existing = BTreeSet::new();

    let resolved = required
        .into_iter()
        .filter_map(|script| {
            existing.insert(script.hash);
            if let Some(borrowed) = provided.get(&script.hash) {
                Some((script, borrowed))
            } else {
                missing.insert(script.hash);
                None
            }
        })
        .collect();

    if !missing.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing));
    }

    let extraneous: BTreeSet<ScriptHash> = provided
        .keys()
        .filter(|k| !existing.contains(k))
        .copied()
        .collect();

    if !extraneous.is_empty() {
        return Err(InvalidScripts::ExtraneousScriptWitnesses(extraneous));
    }

    Ok(resolved)
}

/// Check whether any *unauthorized* extraneous datums are provided in the witness set. This is
/// worth some explanation:
///
/// - Some datums are strictly *required*, and they are the one corresponding to inputs locked by
///   (Plutus) scripts that carry a datum hash (and not an inline datum). For those, the preimage
///   must be provided *somewhere* to be able to execute the script. That somewhere may be the
///   witness set.
///
/// - However, we also enforce that the witness set doesn't contain extraneous datums that we can't
///   correlate back to the transaction body. This is because it isn't otherwise possible to
///   enforce that they don't get dropped by a malicious actors (the witness set isn't part of the
///   signature!).
///
/// - Yet, some datum hashes that appear in the transaction body but that aren't strictly required
///   are still allowed (since it's possible to account for them). This is the case when datum
///   hash digests are present in:
///
///   - transaction outputs
///   - outputs of reference inputs
///
///   It's worth noting that collateral inputs and collateral return aren't considered.
fn fail_on_supplemental_datums<C>(
    context: &mut C,
    required: &BTreeSet<DatumHash>,
    witnessed: &BTreeSet<DatumHash>,
) -> Result<(), InvalidScripts>
where
    C: WitnessSlice,
{
    let supplemental = witnessed
        .difference(required)
        .cloned()
        .collect::<BTreeSet<_>>();

    let extraneous = supplemental
        .difference(&context.allowed_supplemental_datums())
        .cloned()
        .collect::<BTreeSet<_>>();

    if !extraneous.is_empty() {
        return Err(InvalidScripts::ExtraneousSupplementalDatums {
            supplemental,
            extraneous,
        });
    }

    Ok(())
}

/// Fails when there are datum hash digests without matching preimages. The preimage can be found
/// in 3 places:
///
/// - inputs
/// - reference inputs
/// - witness set
///
/// The first two are collected during the inputs sub-rule, and yielded by the context's
/// known_datums. In particular, it's worth noting that inline datums in outputs can't be matched
/// against hashes in the same transaction.
fn fail_on_unmatched_datums<C>(
    context: &mut C,
    required: &BTreeSet<DatumHash>,
    mut witnessed: BTreeSet<DatumHash>,
) -> Result<(), InvalidScripts>
where
    C: WitnessSlice,
{
    let mut provided = context.known_datums().into_keys().collect::<BTreeSet<_>>();
    provided.append(&mut witnessed);

    let missing = required
        .difference(&provided)
        .cloned()
        .collect::<BTreeSet<_>>();

    if !missing.is_empty() {
        return Err(InvalidScripts::MissingRequiredDatums { missing, provided });
    }

    Ok(())
}

fn fail_on_missing_datums(missing: BTreeSet<u32>) -> Result<(), InvalidScripts> {
    if !missing.is_empty() {
        return Err(InvalidScripts::UnspendableInputsNoDatums(missing));
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
    #[test_case(fixture!("8dbd1cfb6d9964575bb62565f9543e22c3a612bac6ef01f21779d469a33a72e0"); "incorrect missing script due to re-serialisation")]
    #[test_case(fixture!("ebd7cda7805bc5b89c0fb3c8ad44f6549ab72c1040eb47019146e3f5f98298e1"); "native script locked with datum")]
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
