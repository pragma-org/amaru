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
    ExUnits, HasExUnits, HasScriptHash, Hash, Language, MemoizedDatum, MemoizedScript, NativeScript, PlutusScript,
    PlutusVersion, ProtocolParameters, ProtocolVersion, RedeemerKey, RedeemerTag, RequiredScript, ScriptIntegrityData,
    ValidityInterval, WitnessSet, redeemer_tag_to_string,
    size::{DATUM, SCRIPT},
    utils::string::display_collection,
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_uplc::{
    arena::Arena,
    flat::{FlatDecodeError, decode_plutus_script},
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

#[derive(Clone, Copy)]
pub(super) enum ProvidedScript<'a> {
    // TODO: Use of 'NativeScript'
    //
    // This should very likely be 'MemoizedNativeScript'; we could likely get rid of the
    // 'NativeScript' entirely now already?
    Native(&'a NativeScript),
    PlutusV1,
    PlutusV2,
    PlutusV3,
}

impl<'a> Deref for ProvidedScript<'a> {
    type Target = ProvidedScript<'a>;
    fn deref(&self) -> &Self::Target {
        self
    }
}

impl TryFrom<&ProvidedScript<'_>> for PlutusVersion {
    type Error = ();
    fn try_from(script: &ProvidedScript<'_>) -> Result<Self, Self::Error> {
        match script {
            ProvidedScript::Native(..) => Err(()),
            ProvidedScript::PlutusV1 => Ok(PlutusVersion::V1),
            ProvidedScript::PlutusV2 => Ok(PlutusVersion::V2),
            ProvidedScript::PlutusV3 => Ok(PlutusVersion::V3),
        }
    }
}

impl TryFrom<&ProvidedScript<'_>> for Language {
    type Error = ();
    fn try_from(script: &ProvidedScript<'_>) -> Result<Self, Self::Error> {
        match script {
            ProvidedScript::Native(..) => Err(()),
            ProvidedScript::PlutusV1 => Ok(Language::PlutusV1),
            ProvidedScript::PlutusV2 => Ok(Language::PlutusV2),
            ProvidedScript::PlutusV3 => Ok(Language::PlutusV3),
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
            redeemer_tag_to_string(&redeemer_key.tag),
            redeemer_key.index
        )).collect::<Vec<_>>().join(", ")
    )]
    ExtraneousRedeemers(Vec<RedeemerKey>),

    #[error(
        "missing redeemers: [{}]",
        .0.iter().map(|redeemer_key| format!(
            "[{}, {}]",
            redeemer_tag_to_string(&redeemer_key.tag),
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

    #[error(
        "script integrity hash mismatch: supplied {supplied:?}, expected {}",
        format_expected_integrity(.expected.as_deref())
    )]
    ScriptIntegrityHashMismatch { supplied: Option<Hash<32>>, expected: Option<Box<ScriptIntegrityData>> },

    #[error("no cost model in protocol parameters for language used by transaction: {0:?}")]
    MissingCostModel(Language),
}

// TODO: Split this whole function into smaller functions to make it more graspable.
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    witness_set: &WitnessSet,
    validity_interval: ValidityInterval,
    protocol_parameters: &ProtocolParameters,
    script_data_hash: Option<Hash<32>>,
) -> Result<(), InvalidScripts>
where
    C: UtxoSlice + WitnessSlice + fmt::Debug,
{
    fail_on_too_many_ex_units(witness_set, protocol_parameters)?;

    let required_scripts = context.required_scripts();

    let required_script_hashes: BTreeSet<&Hash<SCRIPT>> =
        required_scripts.iter().map(|RequiredScript { hash, .. }| hash).collect();

    let provided_scripts = collect_provided_scripts(
        context,
        arena_pool,
        &required_script_hashes,
        witness_set,
        protocol_parameters.protocol_version,
    )?;

    super::native_scripts::execute(&provided_scripts, &required_script_hashes, witness_set, validity_interval)?;

    let required_scripts = fail_on_script_symmetric_differences(required_scripts, &provided_scripts)?;

    let (mut required_redeemers, required_datums) = partition_scripts(required_scripts)?;

    let witnessed_datums = datum_hashes(witness_set);

    let languages: Vec<Language> =
        provided_scripts.values().filter_map(|script| Language::try_from(script).ok()).collect();

    fail_on_supplemental_datums(context, &required_datums, &witnessed_datums)?;

    fail_on_unmatched_datums(context, &required_datums, witnessed_datums)?;

    let mut extra_redeemers = Vec::new();

    if let Some(provided_redemeers) = witness_set.redeemer.as_ref() {
        provided_redemeers.iter_unique().for_each(|(provided, _, _)| {
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

    for lang in &languages {
        let has_cost_model = match lang {
            Language::PlutusV1 => protocol_parameters.cost_models.plutus_v1.is_some(),
            Language::PlutusV2 => protocol_parameters.cost_models.plutus_v2.is_some(),
            Language::PlutusV3 => protocol_parameters.cost_models.plutus_v3.is_some(),
        };
        if !has_cost_model {
            return Err(InvalidScripts::MissingCostModel(lang.clone()));
        }
    }

    // NOTE: Two conformance tests ("PlutusV3 Initialization/Updating CostModels ...") fail this
    // check because they contain multi-epoch test vectors where governance actions update the cost
    // models mid-test. The test harness loads protocol parameters once at the start and doesn't
    // update them at epoch boundaries, so later transactions are validated against stale cost
    // models, producing a different script integrity hash.
    let expected = ScriptIntegrityData::from_witness_set(witness_set, &protocol_parameters.cost_models, &languages);
    let expected_hash = expected.as_ref().map(ScriptIntegrityData::hash);
    if script_data_hash != expected_hash {
        return Err(InvalidScripts::ScriptIntegrityHashMismatch {
            supplied: script_data_hash,
            expected: expected.map(Box::new),
        });
    }

    Ok(())
}

fn format_expected_integrity(expected: Option<&ScriptIntegrityData>) -> String {
    match expected {
        Some(data) => format!("{} (computed from: {data})", data.hash()),
        None => "none".to_string(),
    }
}

fn fail_on_too_many_ex_units(
    witness_set: &WitnessSet,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidScripts> {
    let max = protocol_parameters.max_tx_ex_units;
    let provided = witness_set.total_ex_units();

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
    required_scripts: Vec<(RequiredScript, ProvidedScript<'_>)>,
) -> Result<(Vec<RedeemerKey>, BTreeSet<Hash<DATUM>>), InvalidScripts> {
    let mut required_redeemers = Vec::new();
    let mut required_datums = BTreeSet::new();
    let mut missing_datums = BTreeSet::new();

    required_scripts.iter().for_each(|(required_script, kind)| {
        let RequiredScript { index, datum, hash: _, purpose } = required_script;

        let mut require_redeemer = || required_redeemers.push(RedeemerKey::from(required_script));

        let mut unspendable_without_datum = || {
            if purpose == &RedeemerTag::Spend && matches!(datum, MemoizedDatum::None) {
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
            ProvidedScript::Native(..) => {}

            ProvidedScript::PlutusV1 => {
                require_redeemer();
                unspendable_without_datum();
                require_datum_preimage();
            }

            ProvidedScript::PlutusV2 => {
                require_redeemer();
                unspendable_without_datum();
                require_datum_preimage();
            }

            ProvidedScript::PlutusV3 => {
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
    arena_pool: &ArenaPool,
    required: &BTreeSet<&Hash<SCRIPT>>,
    witness_set: &'a WitnessSet,
    protocol_version: ProtocolVersion,
) -> Result<BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>, InvalidScripts>
where
    C: WitnessSlice,
{
    let mut provided = validate_witness_scripts(arena_pool, witness_set, protocol_version)?;

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
fn fail_on_script_symmetric_differences<'a>(
    required: BTreeSet<RequiredScript>,
    provided: &'_ BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>,
) -> Result<Vec<(RequiredScript, ProvidedScript<'a>)>, InvalidScripts> {
    let mut missing = BTreeSet::new();
    let mut existing = BTreeSet::new();

    let resolved = required
        .into_iter()
        .filter_map(|script| {
            existing.insert(script.hash);
            if let Some(provided) = provided.get(&script.hash) {
                Some((script, *provided))
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
    arena: &Arena,
) -> Result<(), FlatDecodeError> {
    let (_program, decoded_version) = decode_plutus_script(script, protocol_version, arena)?;

    if plutus_version != decoded_version {
        // TODO: Should not be a FlatDecodeError here, but something higher level.
        return Err(FlatDecodeError::Message(format!(
            "mismatch in Plutus version: declared={plutus_version:?}, found={decoded_version:?}"
        )));
    }

    // TODO: Carry decoded programs throughout
    //
    // We decode the script bytes here and, if they're well-formed, again during phase 2 validations.
    // We should decode the script bytes once, and then pass them to phase 2 validation for execution.
    Ok(())
}

/// Validate every Plutus script in the witness set and return the witness set's scripts keyed
/// by hash (native scripts are included as-is; Plutus scripts are included after their bytes
/// successfully decode under the given protocol version). Fails with
/// `MalformedScriptWitnesses` if any Plutus script's flat encoding doesn't decode.
fn validate_witness_scripts<'a>(
    arena_pool: &'_ ArenaPool,
    witness_set: &'a WitnessSet,
    protocol_version: ProtocolVersion,
) -> Result<BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>, InvalidScripts> {
    let mut provided = BTreeMap::new();
    let mut malformed = BTreeSet::new();

    if let Some(scripts) = witness_set.native_script.as_deref() {
        for script in scripts {
            provided.insert(script.script_hash(), ProvidedScript::Native(script.as_ref()));
        }
    }

    let arena = arena_pool.acquire();

    collect_plutus_witness_scripts(
        witness_set.plutus_v1_script.as_deref(),
        PlutusVersion::V1,
        protocol_version,
        &arena,
        &mut provided,
        &mut malformed,
    );

    collect_plutus_witness_scripts(
        witness_set.plutus_v2_script.as_deref(),
        PlutusVersion::V2,
        protocol_version,
        &arena,
        &mut provided,
        &mut malformed,
    );

    collect_plutus_witness_scripts(
        witness_set.plutus_v3_script.as_deref(),
        PlutusVersion::V3,
        protocol_version,
        &arena,
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
    arena: &Arena,
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
    use amaru_kernel::{NonEmptyVec, PROTOCOL_VERSION_10, PlutusScript, WitnessSet};
    use amaru_plutus::arena_pool::ArenaPool;
    use test_case::test_case;

    use super::{InvalidScripts, PlutusVersion};

    /// Well-formedness is decided by the UPLC decoder, so a truncated or empty program is rejected
    /// before it is ever evaluated.
    #[test_case(vec![0xDE, 0xAD]; "truncated program")]
    #[test_case(vec![]; "empty program")]
    fn malformed_plutus_script_rejected(bytes: Vec<u8>) {
        let script: PlutusScript<3> = PlutusScript(bytes.into());
        let arena = amaru_uplc::arena::Arena::new();
        assert!(super::validate_plutus_script(&script, PlutusVersion::V3, PROTOCOL_VERSION_10, &arena).is_err());
    }

    #[test]
    fn malformed_witness_script_detected() {
        let witness_set = WitnessSet {
            plutus_v3_script: Some(NonEmptyVec::singleton(PlutusScript(vec![0xDE, 0xAD].into()))),
            ..WitnessSet::default()
        };

        assert!(matches!(
            super::validate_witness_scripts(&ArenaPool::new(1, 1024), &witness_set, PROTOCOL_VERSION_10),
            Err(InvalidScripts::MalformedScriptWitnesses(ref hashes)) if hashes.len() == 1
        ));
    }

    #[test]
    fn no_scripts_no_malformed() {
        let arena_pool = ArenaPool::new(1, 1024);
        let witness_set = WitnessSet::default();
        let provided = super::validate_witness_scripts(&arena_pool, &witness_set, PROTOCOL_VERSION_10)
            .expect("empty witness set should not produce malformed scripts");
        assert!(provided.is_empty());
    }
}
