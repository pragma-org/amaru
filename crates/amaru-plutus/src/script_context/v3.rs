use amaru_kernel::{DatumOption, Mint, Proposal, ProposalId, StakeCredential, Vote};

use crate::script_context::{
    AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData, PolicyId,
    Redeemer, StakeAddress, TimeRange, TransactionId, TransactionInput, TransactionOutput, Voter,
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
