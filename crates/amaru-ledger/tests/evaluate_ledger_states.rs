// Copyright 2024 PRAGMA
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

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{
        AnyCbor, AuxiliaryData, Bytes, Epoch, EraHistory, Hasher, KeepRaw, MintedTx,
        MintedWitnessSet, TransactionPointer, cbor, network::NetworkName,
        protocol_parameters::ProtocolParameters,
    };
    use amaru_ledger::{
        self, context::DefaultValidationContext, rules::transaction, store::GovernanceActivity,
    };
    use std::{collections::BTreeMap, env, fs, io::Write as _, ops::Deref, path::Path};

    // Tests cases are constructed in build.rs, which generates the test_cases.rs file
    include!(concat!(env!("OUT_DIR"), "/test_cases.rs"));

    fn import_and_evaluate_vector(
        test_data_dir: &Path,
        snapshot: &str,
        pparams_dir: &str,
        expected_result: Result<(), &str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let network = NetworkName::Testnet(1);
        let era_history = network.into();
        let vector_file = fs::read(test_data_dir.join(snapshot))?;
        let record: TestVector = cbor::decode(&vector_file)?;

        let actual = evaluate_vector(record, era_history, &test_data_dir.join(pparams_dir))
            .map_err(|e| e.to_string());
        if let Some(path) = std::env::var_os("AMARU_UPDATE_LEDGER_CONFORMANCE_SNAPSHOT_PATH") {
            // Append to the (toml format) snapshot file that tracks which tests are expected to fail.
            if let Err(error) = actual {
                let mut file = fs::OpenOptions::new().append(true).open(path)?;
                writeln!(
                    &mut file,
                    "{} = {}",
                    toml::Value::String(snapshot.to_string()),
                    toml::Value::String(error),
                )?;
            }
        } else {
            let expected = expected_result.map_err(|e| e.to_string());
            assert_eq!(
                expected, actual,
                "The results of a conformance test have changed."
            );
        }
        Ok(())
    }

    #[derive(cbor::Decode)]
    #[allow(dead_code)]
    struct TestVector {
        #[n(0)]
        config: AnyCbor,
        #[n(1)]
        initial_state: AnyCbor,
        #[n(2)]
        final_state: AnyCbor,
        #[n(3)]
        events: Vec<TestVectorEvent>,
        #[n(4)]
        title: String,
    }

    enum TestVectorEvent {
        Transaction(Bytes, bool, u64),
        #[allow(dead_code)]
        PassTick(u64),
        #[allow(dead_code)]
        PassEpoch(u64),
    }

    impl<'b, C> cbor::decode::Decode<'b, C> for TestVectorEvent {
        fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            d.array()?;
            let variant = d.u16()?;

            match variant {
                0 => Ok(TestVectorEvent::Transaction(
                    d.decode_with(ctx)?,
                    d.decode_with(ctx)?,
                    d.decode_with(ctx)?,
                )),
                1 => Ok(TestVectorEvent::PassTick(d.decode_with(ctx)?)),
                2 => Ok(TestVectorEvent::PassEpoch(d.decode_with(ctx)?)),
                _ => Err(cbor::decode::Error::message(
                    "invalid variant id for TestVectorEvent",
                )),
            }
        }
    }

    // Traverse into the NewEpochState and find the utxos. In practice the initial ledger state for
    // each vector is very simple so we don't need to worry about every field. We really just care
    // about the utxos.
    fn decode_ledger_state<'b>(
        d: &mut cbor::Decoder<'b>,
    ) -> Result<
        (
            DefaultValidationContext,
            &'b cbor::bytes::ByteSlice,
            GovernanceActivity,
        ),
        cbor::decode::Error,
    > {
        let _begin_nes = d.array()?;
        let _epoch_no = d.u64()?;
        d.skip()?; // blocks_made
        d.skip()?; // blocks_made
        let _begin_epoch_state = d.array()?;
        d.skip()?; // begin_account_state
        let _begin_ledger_state = d.array()?;
        let _cert_state = d.array()?;
        let _voting_state = d.array()?;
        d.skip()?; // dreps
        d.skip()?; // committee_state
        let number_of_dormant_epochs: Epoch = d.decode()?;
        d.skip()?; // p_state
        d.skip()?; // d_state
        let _utxo_state = d.array()?;

        let mut utxos_map = BTreeMap::new();
        let utxos_map_count = d.map()?;
        match utxos_map_count {
            Some(n) => {
                for _ in 0..n {
                    let tx_in = d.decode()?;
                    let tx_out = d.decode()?;
                    utxos_map.insert(tx_in, tx_out);
                }
            }
            None => loop {
                let ty = d.datatype()?;
                if ty == cbor::data::Type::Break {
                    break;
                }
                let tx_in = d.decode()?;
                let tx_out = d.decode()?;
                utxos_map.insert(tx_in, tx_out);
            },
        }
        d.skip()?; // deposits
        d.skip()?; // fees

        let _gov_state = d.array()?;
        d.skip()?; // proposals
        d.skip()?; // committee
        d.skip()?; // constitution
        let current_pparams_hash = d.decode()?;
        d.skip()?; //previous_pparams_hash
        d.skip()?; // future_pparams
        d.skip()?; // drep_pulsing_state

        d.skip()?; // stake distr
        d.skip()?; // donation
        d.skip()?; //snapshots
        d.skip()?; // non-myopic
        d.skip()?; // pulsing rewards
        d.skip()?; // pool distribution
        d.skip()?; // stashed

        Ok((
            DefaultValidationContext::new(utxos_map),
            current_pparams_hash,
            GovernanceActivity {
                consecutive_dormant_epochs: u64::from(number_of_dormant_epochs) as u32,
            },
        ))
    }

    fn decode_segregated_parameters(
        dir: &Path,
        hash: &cbor::bytes::ByteSlice,
    ) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
        let pparams_file_path = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .find(|path| {
                path.file_name()
                    .map(|filename| filename.to_str() == Some(&hex::encode(hash.as_ref())))
                    .unwrap_or(false)
            })
            .ok_or("Missing pparams file")?;

        let pparams_file = fs::read(pparams_file_path)?;

        let pparams = cbor::Decoder::new(&pparams_file).decode()?;

        Ok(pparams)
    }

    fn evaluate_vector(
        record: TestVector,
        era_history: &EraHistory,
        pparams_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut decoder = cbor::Decoder::new(&record.initial_state);
        let (mut validation_context, pparams_hash, governance_activity) =
            decode_ledger_state(&mut decoder)?;

        let protocol_parameters = decode_segregated_parameters(pparams_dir, pparams_hash)?;

        for (ix, event) in record.events.into_iter().enumerate() {
            let (tx_bytes, success, slot): (Bytes, bool, u64) = match event {
                TestVectorEvent::Transaction(tx, success, slot) => (tx, success, slot),
                TestVectorEvent::PassTick(..) | TestVectorEvent::PassEpoch(..) => continue,
            };
            let tx: MintedTx<'_> = cbor::decode(tx_bytes.as_slice())?;

            let tx_witness_set: MintedWitnessSet<'_> = tx.witness_set.deref().clone();
            let tx_auxiliary_data =
                Into::<Option<KeepRaw<'_, AuxiliaryData>>>::into(tx.auxiliary_data.clone())
                    .map(|aux_data| Hasher::<256>::hash(aux_data.raw_cbor()));

            let pointer = TransactionPointer {
                slot: slot.into(),
                // Using the loop index here is conterintuitive but ensures that tx pointers will be distinct even if
                // the slots are the same. ultimately the pointers are made up since we do not have real blocks
                transaction_index: ix,
            };

            // Run the transaction against the imported ledger state
            let result = transaction::phase_one::execute(
                &mut validation_context,
                &NetworkName::Preprod,
                &protocol_parameters,
                era_history,
                &governance_activity,
                pointer,
                true,
                tx.body,
                &tx_witness_set,
                tx_auxiliary_data,
            );

            match result {
                Ok(_) if !success => return Err("Expected failure, got success".into()),
                Err(e) if success => {
                    return Err(format!("Expected success, got failure: {}", e).into());
                }
                Ok(..) | Err(..) => (),
            }
        }
        Ok(())
    }
}
