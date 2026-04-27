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

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Deref,
};

use amaru_kernel::{
    ExUnits, HasRedeemers, HasScriptHash, Hash, MemoizedDatum, MemoizedScript, NativeScript, PlutusScript,
    ProtocolParameters, ProtocolVersion, RedeemerKey, RequiredScript, ScriptKind, ScriptPurpose, WitnessSet,
    decode_plutus_script, script_purpose_to_string,
    size::{DATUM, SCRIPT},
    sum_ex_units,
    utils::string::display_collection,
};
use amaru_uplc::{arena::Arena, flat::FlatDecodeError, machine::PlutusVersion};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

// TODO: Unify ProvideScript with ScriptKind
//
// These two types look very similar; and it's likely that they can be unified into one.
pub(super) enum ProvidedScript<'a> {
    Native(&'a NativeScript),
    PlutusV1,
    PlutusV2,
    PlutusV3,
}

impl ProvidedScript<'_> {
    pub(super) fn kind(&self) -> ScriptKind {
        match self {
            Self::Native(_) => ScriptKind::Native,
            Self::PlutusV1 => ScriptKind::PlutusV1,
            Self::PlutusV2 => ScriptKind::PlutusV2,
            Self::PlutusV3 => ScriptKind::PlutusV3,
        }
    }
}

impl<'a> From<PlutusVersion> for ProvidedScript<'a> {
    fn from(version: PlutusVersion) -> ProvidedScript<'a> {
        match version {
            PlutusVersion::V1 => ProvidedScript::PlutusV1,
            PlutusVersion::V2 => ProvidedScript::PlutusV2,
            PlutusVersion::V3 => ProvidedScript::PlutusV3,
        }
    }
}

impl<'a> From<&'a MemoizedScript> for ProvidedScript<'a> {
    fn from(script: &'a MemoizedScript) -> Self {
        match script {
            MemoizedScript::NativeScript(ns) => Self::Native(ns.as_ref()),
            MemoizedScript::PlutusV1Script(_) => Self::PlutusV1,
            MemoizedScript::PlutusV2Script(_) => Self::PlutusV2,
            MemoizedScript::PlutusV3Script(_) => Self::PlutusV3,
        }
    }
}

#[derive(Debug, Error)]
pub enum InvalidScripts {
    #[error("missing required scripts: missing [{}]", display_collection(.0))]
    MissingRequiredScripts(BTreeSet<Hash<SCRIPT>>),

    #[error("extraneous script witnesses: extra [{}]", display_collection(.0))]
    ExtraneousScriptWitnesses(BTreeSet<Hash<SCRIPT>>),

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
    MissingRequiredDatums { missing: BTreeSet<Hash<DATUM>>, provided: BTreeSet<Hash<DATUM>> },

    #[error(
        "extraneous supplemental datums: supplemental: [{}], extraneous: [{}]",
        display_collection(supplemental),
        display_collection(extraneous)
    )]
    ExtraneousSupplementalDatums { supplemental: BTreeSet<Hash<DATUM>>, extraneous: BTreeSet<Hash<DATUM>> },

    #[error(
        "extraneous redeemers: [{}]",
        .0.iter().map(|redeemer_key| format!(
            "[{}, {}]",
            script_purpose_to_string(&redeemer_key.tag),
            redeemer_key.index
        )).collect::<Vec<_>>().join(", ")
    )]
    ExtraneousRedeemers(Vec<RedeemerKey>),

    #[error(
        "missing redeemers: [{}]",
        .0.iter().map(|redeemer_key| format!(
            "[{}, {}]",
            script_purpose_to_string(&redeemer_key.tag),
            redeemer_key.index
        )).collect::<Vec<_>>().join(", ")
    )]
    MissingRedeemers(Vec<RedeemerKey>),

    #[error("malformed script witnesses: [{}]", display_collection(.0))]
    MalformedScriptWitnesses(BTreeSet<Hash<SCRIPT>>),

    #[error("transaction execution units exceeded: provided {provided:?}, max {max:?}")]
    TooManyExUnits { provided: ExUnits, max: ExUnits },

    #[error("native script(s) failed to validate: [{}]", display_collection(.0))]
    ScriptWitnessNotValidatingUTXOW(BTreeSet<Hash<SCRIPT>>),
}

// TODO: Split this whole function into smaller functions to make it more graspable.
pub fn execute<C>(
    context: &mut C,
    witness_set: &WitnessSet,
    validity_interval_start: Option<u64>,
    validity_interval_end: Option<u64>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice + fmt::Debug,
{
    fail_on_too_many_ex_units(witness_set, protocol_parameters)?;

    let required_scripts = context.required_scripts();

    let required_script_hashes: BTreeSet<&Hash<SCRIPT>> =
        required_scripts.iter().map(|RequiredScript { hash, .. }| hash).collect();

    let provided_scripts =
        collect_provided_scripts(context, &required_script_hashes, witness_set, protocol_parameters.protocol_version)?;

    super::native_scripts::execute(
        &provided_scripts,
        &required_script_hashes,
        witness_set,
        validity_interval_start,
        validity_interval_end,
    )?;

    let required_scripts = fail_on_script_symmetric_differences(required_scripts, &provided_scripts)?;

    let (mut required_redeemers, required_datums) = partition_scripts(required_scripts)?;

    let witnessed_datums = datum_hashes(witness_set);

    fail_on_supplemental_datums(context, &required_datums, &witnessed_datums)?;

    fail_on_unmatched_datums(context, &required_datums, witnessed_datums)?;

    let mut extra_redeemers = Vec::new();

    if let Some(provided_redemeers) = witness_set.redeemer.as_ref().map(HasRedeemers::redeemers) {
        provided_redemeers.keys().for_each(|provided| {
            if let Some(index) = required_redeemers.iter().position(|required| required == provided.deref()) {
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

fn fail_on_too_many_ex_units(
    witness_set: &WitnessSet,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidScripts> {
    let max = protocol_parameters.max_tx_ex_units;
    let provided = witness_set
        .redeemer
        .as_ref()
        .map(|r| r.redeemers().values().map(|(ex_units, _)| *ex_units).fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units))
        .unwrap_or(ExUnits { mem: 0, steps: 0 });

    if provided.mem > max.mem || provided.steps > max.steps {
        return Err(InvalidScripts::TooManyExUnits { provided, max });
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
    required_scripts: Vec<(RequiredScript, ScriptKind)>,
) -> Result<(Vec<RedeemerKey>, BTreeSet<Hash<DATUM>>), InvalidScripts> {
    let mut required_redeemers = Vec::new();
    let mut required_datums = BTreeSet::new();
    let mut missing_datums = BTreeSet::new();

    required_scripts.iter().for_each(|(required_script, kind)| {
        let RequiredScript { index, datum, hash: _, purpose } = required_script;

        let mut require_redeemer = || required_redeemers.push(RedeemerKey::from(required_script));

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

        match kind {
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
fn datum_hashes(witness_set: &WitnessSet) -> BTreeSet<Hash<DATUM>> {
    witness_set
        .plutus_data
        .as_deref()
        .map(|datums| datums.iter().map(|datum| datum.hash()).collect::<BTreeSet<_>>())
        .unwrap_or_default()
}

/// Collect all scripts (Native & Plutus) that are **available for evaluation**. This includes:
///
/// - Scripts present in the witness set
/// - Scripts from inputs
/// - Scripts from reference inputs
///
/// It **DOES NOT** include:
///
/// - Scripts from *outputs*
/// - Scripts from auxiliary data
/// - Scripts from collateral inputs
/// - Scripts from collateral return
fn collect_provided_scripts<'a, C>(
    context: &'a mut C,
    required: &BTreeSet<&Hash<SCRIPT>>,
    witness_set: &'a WitnessSet,
    protocol_version: ProtocolVersion,
) -> Result<BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>, InvalidScripts>
where
    C: WitnessSlice,
{
    let mut provided = validate_witness_scripts(witness_set, protocol_version)?;

    // Reference-input scripts are not validated here — they were validated when the producing
    // transaction's outputs went through the output rule. We only include those required by
    // the transaction.
    for (script_hash, script_ref) in context.known_scripts() {
        if required.contains(&script_hash) {
            provided.insert(script_hash, ProvidedScript::from(script_ref));
        }
    }

    Ok(provided)
}

/// Ensures that the required and provided scripts match exactly (i.e. check that they're included
/// in each other).
fn fail_on_script_symmetric_differences(
    required: BTreeSet<RequiredScript>,
    provided: &BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>>,
) -> Result<Vec<(RequiredScript, ScriptKind)>, InvalidScripts> {
    let mut missing = BTreeSet::new();
    let mut existing = BTreeSet::new();

    let resolved = required
        .into_iter()
        .filter_map(|script| {
            existing.insert(script.hash);
            if let Some(provided) = provided.get(&script.hash) {
                Some((script, provided.kind()))
            } else {
                missing.insert(script.hash);
                None
            }
        })
        .collect();

    if !missing.is_empty() {
        return Err(InvalidScripts::MissingRequiredScripts(missing));
    }

    let extraneous: BTreeSet<Hash<SCRIPT>> = provided.keys().filter(|k| !existing.contains(k)).copied().collect();

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
    required: &BTreeSet<Hash<DATUM>>,
    witnessed: &BTreeSet<Hash<DATUM>>,
) -> Result<(), InvalidScripts>
where
    C: WitnessSlice,
{
    let supplemental = witnessed.difference(required).cloned().collect::<BTreeSet<_>>();

    let extraneous = supplemental.difference(&context.allowed_supplemental_datums()).cloned().collect::<BTreeSet<_>>();

    if !extraneous.is_empty() {
        return Err(InvalidScripts::ExtraneousSupplementalDatums { supplemental, extraneous });
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
    required: &BTreeSet<Hash<DATUM>>,
    mut witnessed: BTreeSet<Hash<DATUM>>,
) -> Result<(), InvalidScripts>
where
    C: WitnessSlice,
{
    let mut provided = context.known_datums().into_keys().collect::<BTreeSet<_>>();
    provided.append(&mut witnessed);

    let missing = required.difference(&provided).cloned().collect::<BTreeSet<_>>();

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

/// Attempts to flat decode the script bytes to validate they are well formed.
/// Takes an arena to decode the script into, and then resets it.
pub(crate) fn validate_plutus_script<const V: usize>(
    script: &PlutusScript<V>,
    plutus_version: PlutusVersion,
    protocol_version: ProtocolVersion,
    arena: &mut Arena,
) -> Result<(), FlatDecodeError> {
    arena.reset();

    let (_program, decoded_version) = decode_plutus_script(script, protocol_version, arena)?;

    if plutus_version != decoded_version {
        // TODO: Should not be a FlatDecodeError here, but something higher level.
        return Err(FlatDecodeError::Message(format!(
            "mismatch in Plutus version: declared={plutus_version:?}, found={decoded_version:?}"
        )));
    }

    // FIXME: Carry decoded programs throughout
    //
    // We decode the script bytes here and, if they're well-formed, again during phase 2 validations.
    // We should decode the script bytes once, and then pass them to phase 2 validation for execution.
    Ok(())
}

/// Validate every Plutus script in the witness set and return the witness set's scripts keyed
/// by hash (native scripts are included as-is; Plutus scripts are included after their bytes
/// successfully decode under the given protocol version). Fails with
/// `MalformedScriptWitnesses` if any Plutus script's flat encoding doesn't decode.
fn validate_witness_scripts(
    witness_set: &WitnessSet,
    protocol_version: ProtocolVersion,
) -> Result<BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>>, InvalidScripts> {
    let mut provided = BTreeMap::new();
    let mut malformed = BTreeSet::new();
    let mut arena = Arena::new();

    if let Some(scripts) = witness_set.native_script.as_deref() {
        for script in scripts {
            provided.insert(script.script_hash(), ProvidedScript::Native(script.as_ref()));
        }
    }

    collect_plutus_witness_scripts(
        witness_set.plutus_v1_script.as_deref(),
        PlutusVersion::V1,
        protocol_version,
        &mut arena,
        &mut provided,
        &mut malformed,
    );

    collect_plutus_witness_scripts(
        witness_set.plutus_v2_script.as_deref(),
        PlutusVersion::V2,
        protocol_version,
        &mut arena,
        &mut provided,
        &mut malformed,
    );

    collect_plutus_witness_scripts(
        witness_set.plutus_v3_script.as_deref(),
        PlutusVersion::V3,
        protocol_version,
        &mut arena,
        &mut provided,
        &mut malformed,
    );

    // TODO: Early return of ledger failures
    //
    // It is essential for the ledger to return as early as possible to minimize the amount of work
    // being done. We could potentially adjust this behaviour at a later stage when running in a
    // client mode to provide better errors; but that's not the goal _right now_. Note that I am
    // not changing this now because:
    //
    // 1. I am in the middle of a review and it's not the time; it might break unrelated tests.
    // 2. I would like to do a more extensive pass on the whole ledger regarding this; there might
    //    be more similar occurences.
    //
    // TL; DR; do not decode ALL scripts if one is malformed, return at the first one.
    if !malformed.is_empty() {
        return Err(InvalidScripts::MalformedScriptWitnesses(malformed));
    }

    Ok(provided)
}

fn collect_plutus_witness_scripts<const V: usize>(
    scripts: Option<&[PlutusScript<V>]>,
    plutus_version: PlutusVersion,
    protocol_version: ProtocolVersion,
    arena: &mut Arena,
    provided: &mut BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>>,
    malformed: &mut BTreeSet<Hash<SCRIPT>>,
) {
    let Some(scripts) = scripts else { return };
    for script in scripts {
        let hash = script.script_hash();
        provided.insert(hash, ProvidedScript::from(plutus_version));
        if validate_plutus_script(script, plutus_version, protocol_version, arena).is_err() {
            malformed.insert(hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        ExUnits, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PlutusScript, ProtocolParameters, ProtocolVersion,
        TransactionBody, WitnessSet, include_cbor,
    };
    use test_case::test_case;

    use super::{InvalidScripts, PlutusVersion};
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};

    fn preprod_pv() -> ProtocolVersion {
        PREPROD_DEFAULT_PROTOCOL_PARAMETERS.protocol_version
    }

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/witness.cbor")),
                amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $pp:expr) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                $pp,
            )
        };
    }
    #[test_case(
        fixture!("8dbd1cfb6d9964575bb62565f9543e22c3a612bac6ef01f21779d469a33a72e0");
        "incorrect missing script due to re-serialisation"
    )]
    #[test_case(
        fixture!("ebd7cda7805bc5b89c0fb3c8ad44f6549ab72c1040eb47019146e3f5f98298e1");
        "native script locked with datum"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e");
        "happy path"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "supplemental-datum-output");
        "supplemental datum output"
    )]
    #[test_case(
        fixture!("99cd1c8159255cf384ece25f5516fa54daaee6c5efb3f006ecf9780a0775b1dc");
        "reference script in inputs"
    )]
    #[test_case(
        fixture!("e974fecbf45ac386a76605e9e847a2e5d27c007fdd0be674cbad538e0c35fe01", "required-scripts");
        "proposal script"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-scripts")
        => matches Err(InvalidScripts::MissingRequiredScripts(..));
        "missing required scripts"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-script-witness")
        => matches Err(InvalidScripts::ExtraneousScriptWitnesses(..));
        "extraneous script witness"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "unspendable-input")
        => matches Err(InvalidScripts::UnspendableInputsNoDatums(..));
        "unspendable input"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-datum")
        => matches Err(InvalidScripts::MissingRequiredDatums{..});
        "missing required datum"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-supplemental-datum")
        => matches Err(InvalidScripts::ExtraneousSupplementalDatums{..});
        "extraneous supplemental datum"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "missing-required-redeemer")
        => matches Err(InvalidScripts::MissingRedeemers{..});
        "missing required redeemer"
    )]
    #[test_case(
        fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "extraneous-redeemer")
        => matches Err(InvalidScripts::ExtraneousRedeemers{..});
        "extraneous redeemer"
    )]
    #[test_case(
        fixture!("83036e0c9851c1df44157a8407b1daa34f25549e0644f432e655bd80b0429eba"); "duplicate redeemers"
    )]
    #[test_case(
        fixture!(
            "3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e",
            ProtocolParameters {
                max_tx_ex_units: ExUnits { mem: 1, steps: 1 },
                ..amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()
            }
        ) => matches Err(InvalidScripts::TooManyExUnits{..});
        "too many ex units"
    )]
    fn test_scripts(
        (mut ctx, tx, witness_set, protocol_parameters): (
            AssertValidationContext,
            TransactionBody,
            WitnessSet,
            ProtocolParameters,
        ),
    ) -> Result<(), InvalidScripts> {
        super::execute(
            &mut ctx,
            &witness_set,
            tx.validity_interval_start,
            tx.validity_interval_end,
            &protocol_parameters,
        )
    }

    #[test]
    fn malformed_script_rejected() {
        let script: PlutusScript<3> = PlutusScript(vec![0xDE, 0xAD].into());
        let mut arena = amaru_uplc::arena::Arena::new();
        assert!(super::validate_plutus_script(&script, PlutusVersion::V3, preprod_pv(), &mut arena).is_err());
    }

    #[test]
    fn empty_script_rejected() {
        let script: PlutusScript<3> = PlutusScript(vec![].into());
        let mut arena = amaru_uplc::arena::Arena::new();
        assert!(super::validate_plutus_script(&script, PlutusVersion::V3, preprod_pv(), &mut arena).is_err());
    }

    #[test]
    fn malformed_witness_script_detected() {
        use amaru_kernel::{NonEmptyVec, PlutusScript};

        let witness_set = WitnessSet {
            plutus_v3_script: Some(NonEmptyVec::singleton(PlutusScript(vec![0xDE, 0xAD].into()))),
            ..WitnessSet::default()
        };

        assert!(matches!(
            super::validate_witness_scripts(&witness_set, preprod_pv()),
            Err(InvalidScripts::MalformedScriptWitnesses(ref hashes)) if hashes.len() == 1
        ));
    }

    #[test]
    fn no_scripts_no_malformed() {
        let witness_set = WitnessSet::default();
        let provided = super::validate_witness_scripts(&witness_set, preprod_pv())
            .expect("empty witness set should not produce malformed scripts");
        assert!(provided.is_empty());
    }
}
