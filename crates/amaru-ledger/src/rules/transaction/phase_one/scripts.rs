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
    ExUnits, HasExUnits, HasScriptHash, Hash, MemoizedDatum, MemoizedScript, NativeScript, PlutusScript, PlutusVersion,
    ProtocolParameters, RedeemerKey, RedeemerTag, RequiredScript, ScriptIntegrityData, ValidityInterval, WitnessSet,
    size::{DATUM, SCRIPT},
    utils::string::display_collection,
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

impl<'a> From<PlutusVersion> for ProvidedScript<'a> {
    fn from(version: PlutusVersion) -> Self {
        match version {
            PlutusVersion::V1 => Self::PlutusV1,
            PlutusVersion::V2 => Self::PlutusV2,
            PlutusVersion::V3 => Self::PlutusV3,
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
            redeemer_key.tag,
            redeemer_key.index
        )).collect::<Vec<_>>().join(", ")
    )]
    ExtraneousRedeemers(Vec<RedeemerKey>),

    #[error(
        "missing redeemers: [{}]",
        .0.iter().map(|redeemer_key| format!(
            "[{}, {}]",
            redeemer_key.tag,
            redeemer_key.index
        )).collect::<Vec<_>>().join(", ")
    )]
    MissingRedeemers(Vec<RedeemerKey>),

    #[error("transaction execution units exceeded: provided {provided:?}, max {max:?}")]
    TooManyExUnits { provided: ExUnits, max: ExUnits },

    #[error("native script(s) failed to validate: [{}]", display_collection(.0))]
    ScriptWitnessNotValidatingUTXOW(BTreeSet<Hash<SCRIPT>>),

    #[error(
        "script integrity hash mismatch: supplied {supplied:?}, expected {}",
        format_expected_integrity(.expected.as_deref())
    )]
    ScriptIntegrityHashMismatch { supplied: Option<Hash<32>>, expected: Option<Box<ScriptIntegrityData>> },
}

// TODO: Split this whole function into smaller functions to make it more graspable.
pub fn execute<C>(
    context: &mut C,
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

    let (provided_scripts, witnessed_script_hashes, reference_script_hashes) =
        collect_provided_scripts(context, &required_script_hashes, witness_set);

    super::native_scripts::execute(&provided_scripts, &required_script_hashes, witness_set, validity_interval)?;

    let required_scripts = fail_on_script_symmetric_differences(
        required_scripts,
        &provided_scripts,
        &witnessed_script_hashes,
        &reference_script_hashes,
    )?;

    let (mut required_redeemers, required_datums) = partition_scripts(required_scripts)?;

    let witnessed_datums = datum_hashes(witness_set);

    let languages: Vec<PlutusVersion> =
        provided_scripts.values().filter_map(|script| PlutusVersion::try_from(script).ok()).collect();

    fail_on_supplemental_datums(context, &required_datums, &witnessed_datums)?;

    fail_on_unmatched_datums(context, &required_datums, witnessed_datums)?;

    let mut extra_redeemers = Vec::new();

    if let Some(provided_redemeers) = witness_set.redeemer.as_ref() {
        provided_redemeers.iter_unique().for_each(|(provided, _, _)| {
            if let Some(index) = required_redeemers.iter().position(|required| required == provided.deref()) {
                required_redeemers.remove(index);
            } else {
                extra_redeemers.push(*provided.deref());
            }
        })
    }

    if !required_redeemers.is_empty() {
        return Err(InvalidScripts::MissingRedeemers(required_redeemers));
    }

    if !extra_redeemers.is_empty() {
        return Err(InvalidScripts::ExtraneousRedeemers(extra_redeemers));
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
                required_datums.insert(*hash.as_ref());
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
    required: &BTreeSet<&Hash<SCRIPT>>,
    witness_set: &'a WitnessSet,
) -> (BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>, BTreeSet<Hash<SCRIPT>>, BTreeSet<Hash<SCRIPT>>)
where
    C: WitnessSlice,
{
    let mut provided = collect_witness_scripts(witness_set);
    let witnessed = provided.keys().copied().collect();
    let mut referenced = BTreeSet::new();

    for (script_hash, script_ref) in context.known_scripts() {
        if required.contains(&script_hash) {
            referenced.insert(script_hash);
            provided.insert(script_hash, ProvidedScript::from(script_ref));
        }
    }

    (provided, witnessed, referenced)
}

/// Ensures that every required script is available and that the witness set contains exactly the
/// required scripts which are not supplied by a reference input.
fn fail_on_script_symmetric_differences<'a>(
    required: BTreeSet<RequiredScript>,
    provided: &'_ BTreeMap<Hash<SCRIPT>, ProvidedScript<'a>>,
    witnessed: &BTreeSet<Hash<SCRIPT>>,
    referenced: &BTreeSet<Hash<SCRIPT>>,
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

    let extraneous = witnessed
        .iter()
        .filter(|hash| !existing.contains(hash) || referenced.contains(hash))
        .copied()
        .collect::<BTreeSet<_>>();

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

/// Collect the witness set's scripts keyed by hash.
fn collect_witness_scripts(witness_set: &WitnessSet) -> BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>> {
    let mut provided = BTreeMap::new();

    if let Some(scripts) = witness_set.native_script.as_deref() {
        for script in scripts {
            provided.insert(script.script_hash(), ProvidedScript::Native(script.as_ref()));
        }
    }

    collect_plutus_witness_scripts(witness_set.plutus_v1_script.as_deref(), PlutusVersion::V1, &mut provided);

    collect_plutus_witness_scripts(witness_set.plutus_v2_script.as_deref(), PlutusVersion::V2, &mut provided);

    collect_plutus_witness_scripts(witness_set.plutus_v3_script.as_deref(), PlutusVersion::V3, &mut provided);

    provided
}

fn collect_plutus_witness_scripts<const V: usize>(
    scripts: Option<&[PlutusScript<V>]>,
    plutus_version: PlutusVersion,
    provided: &mut BTreeMap<Hash<SCRIPT>, ProvidedScript<'_>>,
) {
    let Some(scripts) = scripts else { return };
    for script in scripts {
        let hash = script.script_hash();
        provided.insert(hash, ProvidedScript::from(plutus_version));
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::WitnessSet;

    #[test]
    fn no_witness_scripts() {
        let witness_set = WitnessSet::default();
        let provided = super::collect_witness_scripts(&witness_set);
        assert!(provided.is_empty());
    }
}
