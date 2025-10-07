use amaru_kernel::{
    AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, PlutusData, PolicyId, Redeemer,
    StakeAddress, TransactionId, TransactionInput, TransactionOutput, Value, Voter, Withdrawal,
};

use amaru_slot_arithmetic::TimeMs;

pub mod v1;
pub mod v2;
pub mod v3;

pub use v1::ScriptContext as ScriptContextV1;
pub use v1::TxInfo as TxInfoV1;
pub use v2::ScriptContext as ScriptContextV2;
pub use v2::TxInfo as TxInfoV2;
pub use v3::TxInfo as TxInfoV3;

pub struct OutputRef {
    pub input: TransactionInput,
    pub output: TransactionOutput,
}

pub struct TimeRange {
    pub lower_bound: Option<TimeMs>,
    pub upper_bound: Option<TimeMs>,
}
