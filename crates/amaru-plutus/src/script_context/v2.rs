use amaru_kernel::{Address, DatumOption, ScriptRef, from_alonzo_value};

use crate::{
    Constr, DEFAULT_TAG, MaybeIndefArray, ToConstrTag, ToPlutusData, constr,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData,
        Redeemer, StakeAddress, TimeRange, TransactionId, TransactionOutput, Value,
        v1::ScriptPurpose,
    },
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

impl ToPlutusData<2> for TxInfo {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 2, 0, self.inputs, self.reference_inputs, self.outputs, self.fee, self.mint, self.certificates, self.withdrawals, self.valid_range, self.signatories,  self.redeemers, self.data, constr!(v:2,0, self.id))
    }
}

impl ToPlutusData<2> for TransactionOutput {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                constr!(v: 2, 0, Address::from_bytes(&output.address).unwrap(), from_alonzo_value(&output.amount).expect("illegal alonzo value"), None::<DatumOption>, None::<ScriptRef>)
            }
            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                constr!(v: 2, 0, Address::from_bytes(&output.address).unwrap(), output.value, output.datum_option, output.script_ref.as_ref().map(|s| s.clone().unwrap()))
            }
        }
    }
}

impl ToPlutusData<2> for OutputRef {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 2, 0, self.input, self.output)
    }
}
