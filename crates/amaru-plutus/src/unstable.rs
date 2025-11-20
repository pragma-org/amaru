use std::{collections::BTreeMap, sync::Arc};

use amaru_kernel::{
    DatumHash, MemoizedPlutusData, MemoizedTransactionOutput, TransactionInput,
    arc_mapped::ArcMapped,
};

/// A TxInfoStorage type to store owned data
/// Which allows the `TxInfo` type to still use references
///
/// Obviously, it must out live the `TxInfo` that references it
#[derive(Debug, Default)]
pub struct TxInfoStorage {
    inputs: BTreeMap<TransactionInput, Arc<MemoizedTransactionOutput>>,
    outputs: Vec<Arc<MemoizedTransactionOutput>>,
    datums: BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>,
}

impl TxInfoStorage {
    pub fn add_input(&mut self, input: TransactionInput, output: Arc<MemoizedTransactionOutput>) {
        self.inputs.insert(input, output);
    }

    pub fn add_output(&mut self, output: Arc<MemoizedTransactionOutput>) {
        self.outputs.push(output);
    }

    pub fn set_datums(
        &mut self,
        datums: BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>,
    ) {
        self.datums = datums
    }

    pub fn datums(
        &self,
    ) -> &BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>> {
        &self.datums
    }
}
