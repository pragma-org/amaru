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

use std::collections::HashSet;
use amaru_kernel::{TransactionInput, TransactionOutput, Tx};

pub trait State {
    type Error;
    fn utxo(&self, input: &TransactionInput) -> Result<TransactionOutput, Self::Error>;
}

pub trait Mempool {
    type AddTxError;
    fn add_tx<S: State>(&mut self, state: &S, tx: Tx) -> Result<(), Self::AddTxError>;
    fn make_block<S: State>(&mut self, state: &S) -> Option<Vec<Tx>>;
    fn invalidate_utxos(&mut self, txins: HashSet<&TransactionInput>);
}

#[derive(Clone, Debug)]
pub struct SimpleMempool {
    transactions: Vec<Tx>,
}

impl SimpleMempool {
    pub fn new() -> Self {
        SimpleMempool {
            transactions: vec![],
        }
    }

    // Just check that each tx input exists
    fn validate_tx<S: State>(&self, state: &S, tx: &Tx) -> bool {
        tx.transaction_body.inputs.iter().all(|input| state.utxo(input).is_ok())
    }
}

impl Mempool for SimpleMempool {
    type AddTxError = &'static str;
    fn add_tx<S: State>(&mut self, snapshot: &S, tx: Tx) -> Result<(), Self::AddTxError> {
        if !self.validate_tx(snapshot, &tx) {
            return Err("tx did not validate");
        }
        self.transactions.push(tx);
        Ok(())
    }

    fn make_block<S: State>(&mut self, _snapshot: &S) -> Option<Vec<Tx>> {
        Some(self.transactions.clone())
    }

    fn invalidate_utxos(&mut self, txins: HashSet<&TransactionInput>) {
        self.transactions.retain(|tx| {
            tx.transaction_body
                .inputs
                .iter()
                .all(|input| !txins.contains(input))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pallas_primitives::{TransactionInput};
    use pallas_primitives::conway::{TransactionOutput, Tx};
    use std::collections::HashMap;

    struct TestState {
        utxos: HashMap<TransactionInput, TransactionOutput>,
    }

    impl State for TestState {
        type Error = TestStateError;
        fn utxo(&self, input: &TransactionInput) -> Result<TransactionOutput, TestStateError> {
            self.utxos.get(input).ok_or(TestStateError{}).cloned()
        }
    }

    #[derive(Debug)]
    struct TestStateError {
    }

    impl std::fmt::Display for TestStateError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
            write!(f, "TestStateError")
        }
    }

    impl std::error::Error for TestStateError {
    }

    fn make_empty_tx() -> Tx {
        Tx {
            transaction_body: pallas_primitives::conway::TransactionBody {
                inputs: vec![].into(),
                outputs: vec![].into(),
                fee: 0,
                ttl: None,
                validity_interval_start: None,
                certificates: None,
                withdrawals: None,
                auxiliary_data_hash: None,
                mint: None,
                script_data_hash: None,
                collateral: None,
                required_signers: None,
                network_id: None,
                collateral_return: None,
                reference_inputs: None,
                total_collateral: None,
                voting_procedures: None,
                proposal_procedures: None,
                treasury_value: None,
                donation: None,
            },
            transaction_witness_set: pallas_primitives::conway::WitnessSet {
                vkeywitness: None,
                native_script: None,
                bootstrap_witness: None,
                plutus_v1_script: None,
                plutus_data: None,
                redeemer: None,
                plutus_v2_script: None,
                plutus_v3_script: None,
            },
            success: true,
            auxiliary_data: None.into(),
        }
    }

    fn simple_output(amount: u64) -> TransactionOutput {
        pallas_primitives::conway::PseudoTransactionOutput::PostAlonzo(
             pallas_primitives::conway::PostAlonzoTransactionOutput {
                 address: vec![].into(),
                 value: pallas_primitives::conway::Value::Coin(amount),
                 datum_option: None,
                 script_ref: None,
             },
         )
    }

    fn tx_add_input(tx: &mut Tx, input: TransactionInput) {
        let mut inputs: Vec<TransactionInput> = tx.transaction_body.inputs.clone().to_vec();
        inputs.push(input);
        tx.transaction_body.inputs = inputs.into();
    }

    #[test]
    fn add_tx_success() {
        let mut mempool = SimpleMempool::new();
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let state = TestState {
            utxos: utxos,
        };
        let basic_tx = make_empty_tx();
        let success = mempool.add_tx(&state, basic_tx);
        assert_eq!(success.is_ok(), true);
    }

    #[test]
    fn add_tx_failure() {
        let mut mempool = SimpleMempool::new(); 
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let state = TestState {
            utxos: utxos,
        };
        let mut basic_tx = make_empty_tx();
        tx_add_input(&mut basic_tx, TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 1,
        });
        let success = mempool.add_tx(&state, basic_tx);
        assert_eq!(success.is_ok(), false);
    }

    #[test]
    fn invalidate_utxos() {
        let mut mempool = SimpleMempool {
            transactions: vec![],
        };
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let state = TestState {
            utxos: utxos,
        };
        let mut basic_tx = make_empty_tx();
        tx_add_input(&mut basic_tx, TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 0,
        });
        let _ = mempool.add_tx(&state, basic_tx);
        let mut set = HashSet::new();
        set.insert(TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 0,
        });
        mempool.invalidate_utxos(set);
        assert_eq!(mempool.transactions.len(), 0);
    }
}
