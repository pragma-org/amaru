// Copyright 2026 PRAGMA
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

use std::collections::BTreeMap;

use crate::{
    AsShelley, BorrowedScript, Certificate, GovernanceAction, HasOwnership, Hash, OutputReference, PlutusData,
    PlutusMint, PlutusVotes, PlutusWithdrawals, Proposal, RedeemerKey, RedeemerTag, StakeCredential, TransactionInput,
    Voter,
    size::{CREDENTIAL, SCRIPT},
};

/// A [`ScriptInfo`] naming what a script validates, with no spending datum (`T = ()`).
///
/// The resolved purpose as it appears keyed in a transaction's redeemers and in the
/// pre-V3 script-context encoding. [`ScriptPurpose::to_script_info`] attaches a datum to the spend case to
/// produce the V3 `ScriptInfo<Option<&PlutusData>>`.
///
/// Not to be confused with [`crate::ScriptPurpose`], the bare redeemer *tag*; this is that
/// tag *resolved* to the concrete item it points at.
pub type ScriptPurpose<'a> = ScriptInfo<'a, ()>;

/// What a script is validating, generic over the payload carried by its spending case.
///
/// One variant per redeemer kind: minting, spending, rewarding, certifying, voting,
/// proposing. The type parameter `T` rides only on [`Spending`](Self::Spending) and is the
/// sole difference between this enum's two specialisations:
///
/// - [`ScriptPurpose`] (`T = ()`): names *what* is validated, with no spending datum. The
///   redeemer-map view and the pre-V3 encoding.
/// - `ScriptInfo<Option<&PlutusData>>`: the spend case additionally carries its resolved
///   datum. The form a Plutus V3 script receives in its context.
#[derive(Debug, Clone)]
pub enum ScriptInfo<'a, T: Clone> {
    Minting(Hash<CREDENTIAL>),
    Spending(&'a TransactionInput, T),
    Rewarding(StakeCredential),
    Certifying(usize, &'a Certificate),
    Voting(&'a Voter),
    Proposing(usize, &'a Proposal),
}

impl<'a> ScriptPurpose<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn builder(
        key: &RedeemerKey,
        inputs: &[OutputReference<'a>],
        mint: &PlutusMint<'a>,
        withdrawals: &PlutusWithdrawals,
        certs: &[&'a Certificate],
        proposal_procedures: &[&'a Proposal],
        votes: &PlutusVotes<'a>,
        scripts: &BTreeMap<Hash<SCRIPT>, BorrowedScript<'a>>,
    ) -> Option<(Self, BorrowedScript<'a>)> {
        let index = key.index as usize;
        match key.tag {
            RedeemerTag::Spend => inputs.get(index).and_then(|OutputReference { input, output }| {
                if let Some(StakeCredential::ScriptHash(hash)) = output.address.as_shelley().map(|addr| addr.owner()) {
                    scripts.get(&hash).map(|script| (ScriptPurpose::Spending(input, ()), script.clone()))
                } else {
                    None
                }
            }),
            RedeemerTag::Mint => mint.0.keys().nth(index).copied().and_then(|policy_id| {
                scripts.get(&policy_id).map(|script| (ScriptPurpose::Minting(policy_id), script.clone()))
            }),
            RedeemerTag::Reward => withdrawals.keys().nth(index).and_then(|account| {
                if let StakeCredential::ScriptHash(hash) = account.credential() {
                    scripts
                        .get(&hash)
                        .map(|script| (ScriptPurpose::Rewarding(StakeCredential::ScriptHash(hash)), script.clone()))
                } else {
                    None
                }
            }),
            RedeemerTag::Cert => certs.get(index).and_then(|certificate| {
                if let StakeCredential::ScriptHash(hash) = certificate.owner() {
                    scripts.get(&hash).map(|script| (ScriptPurpose::Certifying(index, certificate), script.clone()))
                } else {
                    None
                }
            }),
            RedeemerTag::Vote => votes.0.keys().nth(index).and_then(|voter| {
                if let StakeCredential::ScriptHash(hash) = voter.owner() {
                    scripts.get(&hash).map(|script| (ScriptPurpose::Voting(voter), script.clone()))
                } else {
                    None
                }
            }),
            RedeemerTag::Propose => proposal_procedures.get(index).and_then(|proposal| {
                use GovernanceAction::*;

                let script_hash = match proposal.gov_action {
                    ParameterChange(_, _, Some(gov_proposal_hash)) => Some(gov_proposal_hash),
                    TreasuryWithdrawals(_, Some(gov_proposal_hash)) => Some(gov_proposal_hash),
                    ParameterChange(..)
                    | HardForkInitiation(..)
                    | TreasuryWithdrawals(..)
                    | NoConfidence(_)
                    | UpdateCommittee(..)
                    | NewConstitution(..)
                    | Information => None,
                };

                script_hash.and_then(|hash| {
                    scripts.get(&hash).map(|script| (ScriptPurpose::Proposing(index, proposal), script.clone()))
                })
            }),
        }
    }

    pub fn to_script_info(&self, data: Option<&'a PlutusData>) -> ScriptInfo<'a, Option<&'a PlutusData>> {
        match self {
            ScriptInfo::Spending(input, _) => ScriptInfo::Spending(input, data),
            ScriptInfo::Minting(p) => ScriptInfo::Minting(*p),
            ScriptInfo::Rewarding(s) => ScriptInfo::Rewarding(*s),
            ScriptInfo::Certifying(i, c) => ScriptInfo::Certifying(*i, c),
            ScriptInfo::Voting(v) => ScriptInfo::Voting(v),
            ScriptInfo::Proposing(i, p) => ScriptInfo::Proposing(*i, p),
        }
    }
}
