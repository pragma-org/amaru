use crate::script_context::{
    AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData, Redeemer,
    StakeAddress, TimeRange, TransactionId, TransactionOutput, Value, v1::ScriptPurpose,
};

pub use crate::script_context::v1::ScriptContext;

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V2/Contexts.hs#L82
pub struct TxInfo {
    inputs: Vec<OutputRef>,
    reference_inputs: Vec<OutputRef>,
    outputs: Vec<TransactionOutput>,
    fee: Value,
    mint: Value,
    certificates: Vec<Certificate>,
    withdrawals: KeyValuePairs<StakeAddress, Lovelace>,
    valid_range: TimeRange,
    signatories: Vec<AddrKeyhash>,
    redeemers: KeyValuePairs<ScriptPurpose, Redeemer>,
    data: KeyValuePairs<DatumHash, PlutusData>,
    id: TransactionId,
}
