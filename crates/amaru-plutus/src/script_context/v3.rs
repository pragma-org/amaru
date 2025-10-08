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

use amaru_kernel::{
    Address, DatumOption, Mint, Proposal, ProposalId, StakeCredential, Value, Vote,
};

use crate::{
    Constr, DEFAULT_TAG, MaybeIndefArray, ToConstrTag, ToPlutusData, constr,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData,
        PolicyId, Redeemer, StakeAddress, TimeRange, TransactionId, TransactionInput,
        TransactionOutput, Voter,
    },
};

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V3/Data/Contexts.hs#L572
pub struct TxInfo {
    inputs: Vec<OutputRef>,
    reference_inputs: Vec<OutputRef>,
    outputs: Vec<TransactionOutput>,
    fee: Lovelace,
    mint: Mint,
    certificates: Vec<Certificate>,
    withdrawals: KeyValuePairs<StakeAddress, Lovelace>,
    valid_range: TimeRange,
    signatories: Vec<AddrKeyhash>,
    // TODO: figure out what generic goes here
    redeemers: KeyValuePairs<ScriptInfo, Redeemer>,
    data: KeyValuePairs<DatumHash, PlutusData>,
    id: TransactionId,
    votes: KeyValuePairs<Voter, KeyValuePairs<ProposalId, Vote>>,
    proposal_procedures: Vec<Proposal>,
    current_treasury_amount: Option<Lovelace>,
    treasury_donation: Option<Lovelace>,
}

#[derive(Clone)]
pub enum ScriptInfo {
    Minting(PolicyId),
    Spending(TransactionInput, Option<DatumOption>),
    Rewarding(StakeCredential),
    Certifying(usize, Certificate),
    Voting(Voter),
    Proposing(usize, Proposal),
}

pub struct ScriptContext {
    tx_info: TxInfo,
    redeemer: Redeemer,
    script_info: ScriptInfo,
}

impl ToPlutusData<3> for TransactionInput {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.transaction_id, self.index)
    }
}

impl ToPlutusData<3> for TransactionOutput {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            amaru_kernel::PseudoTransactionOutput::Legacy(transaction_output) => todo!(),
            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                constr!(v: 3, 0, Address::from_bytes(&output.address).unwrap(), output.value, output.datum_option, output.script_ref.as_ref().map(|s| s.clone().unwrap()))
            }
        }
    }
}

impl ToPlutusData<3> for Value {
    fn to_plutus_data(&self) -> PlutusData {
        todo!()
    }
}
