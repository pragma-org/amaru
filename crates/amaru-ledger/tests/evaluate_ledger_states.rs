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
    use std::{
        collections::BTreeMap,
        env, fs,
        io::Write as _,
        path::{Path, PathBuf},
        sync::LazyLock,
    };

    use amaru_kernel::{
        Account, Bytes, CertificatePointer, Constitution, ConstitutionalCommittee, ConstitutionalCommitteeMemberStatus,
        DRepRegistration, DRepState, Epoch, EraHistory, Lovelace, MemoizedTransactionOutput, NetworkName,
        PROTOCOL_VERSION_10, Point, PoolId, PoolParams, PoolSlim, ProposalId, ProposalSlim,
        ProposalState as NewEpochProposalState, ProtocolParameters, Slot, StakeCredential, Transaction,
        TransactionInput, TransactionPointer, WitnessSet, cbor, cbor as minicbor, utils::cbor::SerialisedAsArray,
    };
    use amaru_ledger::{
        self,
        context::{AccountState, DefaultValidationContext, ProposalStateSlim},
        epoch_transition::GovernanceActivity,
        rules::transaction,
        snapshot,
    };
    use amaru_plutus::arena_pool::ArenaPool;

    // Tests cases are constructed in build.rs, which generates the test_cases.rs file
    include!(concat!(env!("OUT_DIR"), "/test_cases.rs"));

    static PPARAMS_DIR: LazyLock<PathBuf> =
        LazyLock::new(|| ["tests", "data", "rules-conformance", "pparams"].iter().collect());

    fn import_and_evaluate_vector(
        test_data_dir: &Path,
        snapshot: &str,
        expected_result: Result<(), &str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let vector_file = fs::read(test_data_dir.join(snapshot))?;
        let record: TestVector = cbor::decode(&vector_file)?;
        let actual = evaluate_vector(record, &EraHistory::default(), PPARAMS_DIR.as_ref()).map_err(|e| e.to_string());
        if let Some(path) = std::env::var_os("AMARU_UPDATE_LEDGER_CONFORMANCE_SNAPSHOT_PATH") {
            // Append to the (toml format) snapshot file that tracks which tests are expected to fail.
            if let Err(error) = actual {
                let mut file = fs::OpenOptions::new().append(true).open(path)?;
                writeln!(&mut file, "{} = {}", toml::Value::String(snapshot.to_string()), toml::Value::String(error),)?;
            }
        } else {
            let expected = expected_result.map_err(|e| e.to_string());
            assert_eq!(expected, actual, "The results of a conformance test have changed.");
        }
        Ok(())
    }

    #[derive(cbor::Decode)]
    #[cbor(context_bound = "cbor::HasProtocolVersion")]
    #[allow(dead_code)]
    struct TestVector {
        #[n(0)]
        config: cbor::Skip,
        #[n(1)]
        initial_state: cbor::Skip,
        #[n(2)]
        final_state: cbor::Skip,
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

    impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for TestVectorEvent {
        fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            d.array()?;
            let variant = d.u16()?;

            match variant {
                0 => Ok(TestVectorEvent::Transaction(d.decode_with(ctx)?, d.decode_with(ctx)?, d.decode_with(ctx)?)),
                1 => Ok(TestVectorEvent::PassTick(d.decode_with(ctx)?)),
                2 => Ok(TestVectorEvent::PassEpoch(d.decode_with(ctx)?)),
                _ => Err(cbor::decode::Error::message("invalid variant id for TestVectorEvent")),
            }
        }
    }

    // Decode the NewEpochState's initial ledger state into the pieces a ValidationContext needs.
    // Conformance vectors currently carry only UTxO state; the other sections decode as empty.
    struct DecodedLedgerState<'b> {
        utxos: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
        pools: BTreeMap<PoolId, PoolSlim>,
        accounts: BTreeMap<StakeCredential, Account>,
        dreps: BTreeMap<StakeCredential, DRepState>,
        cc_members: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus>,
        cc_state: Option<ConstitutionalCommittee>,
        proposals: Vec<NewEpochProposalState>,
        roots: [Option<ProposalId>; 4],
        pparams_hash: &'b cbor::bytes::ByteSlice,
        dormant_epochs: Epoch,
        treasury: Lovelace,
        constitution: Constitution,
    }

    fn decode_ledger_state<'b>(d: &mut cbor::Decoder<'b>) -> Result<DecodedLedgerState<'b>, cbor::decode::Error> {
        let _begin_nes = d.array()?;
        let _epoch_no = d.u64()?;
        d.skip()?; // blocks_made (previous)
        d.skip()?; // blocks_made (current)
        let _begin_epoch_state = d.array()?;
        // account_state (ChainAccountState) is `[treasury, reserves]`; only treasury is read.
        d.array()?;
        let treasury: Lovelace = d.decode()?;
        d.skip()?; // reserves
        let _begin_ledger_state = d.array()?;
        let _cert_state = d.array()?;
        let _voting_state = d.array()?;
        let dreps = d.decode()?;
        let cc_members = d.decode()?;
        let dormant_epochs: Epoch = d.decode()?;

        // PState: [stake pools, future pools, retiring, deposits]. Only pool existence matters here.
        let pools = {
            d.array()?;
            let current_pools: BTreeMap<PoolId, PoolParams> = d.decode()?;
            let future_pools: BTreeMap<PoolId, PoolParams> = d.decode()?;
            d.skip()?; // retiring
            d.skip()?; // deposits
            current_pools
                .into_iter()
                .map(|(pool_id, params)| {
                    let has_pending_updates = future_pools.contains_key(&pool_id);
                    (pool_id, PoolSlim { vrf: params.vrf, has_pending_updates })
                })
                .collect()
        };

        // DState: [unified map, future gen delegs, gen delegs, instantaneous rewards]. The unified map
        // is [accounts, pointers], where each account entry is what cardano-ledger calls a UMElem
        // (reward and deposit, pointers, pool and drep delegation), whose layout the kernel `Account`
        // type mirrors.

        // NOTE: Uncovered branch
        //
        // Every conformance vector has an empty account map, so this UMElem
        // decoding is unexercised here, verified only by construction against the ledger encoding.
        let accounts = {
            let len = d.array()?;
            let _umap_len = d.array()?;
            let accounts: BTreeMap<StakeCredential, Account> = d.decode()?;
            d.skip()?; // pointers
            for _ in 1..len.unwrap_or(0) {
                d.skip()?;
            }
            accounts
        };

        let _utxo_state = d.array()?;

        let mut utxos = BTreeMap::new();
        let utxos_count = d.map()?;
        match utxos_count {
            Some(n) => {
                for _ in 0..n {
                    let tx_in = d.decode()?;
                    let tx_out = d.decode()?;
                    utxos.insert(tx_in, tx_out);
                }
            }
            None => loop {
                let ty = d.datatype()?;
                if ty == cbor::data::Type::Break {
                    break;
                }
                let tx_in = d.decode()?;
                let tx_out = d.decode()?;
                utxos.insert(tx_in, tx_out);
            },
        }
        d.skip()?; // deposits
        d.skip()?; // fees

        let _gov_state = d.array()?;
        // The proposals field nests the governance roots ahead of the proposals themselves.
        d.array()?;
        d.array()?;
        let roots = [
            d.decode::<SerialisedAsArray<_>>()?.0,
            d.decode::<SerialisedAsArray<_>>()?.0,
            d.decode::<SerialisedAsArray<_>>()?.0,
            d.decode::<SerialisedAsArray<_>>()?.0,
        ];
        let proposals = d.decode()?;
        let cc_state = d.decode::<SerialisedAsArray<_>>()?.0;
        let constitution: Constitution = d.decode()?;
        let pparams_hash = d.decode()?;
        d.skip()?; // previous_pparams_hash
        d.skip()?; // future_pparams
        d.skip()?; // drep_pulsing_state

        d.skip()?; // stake distr
        d.skip()?; // donation
        d.skip()?; // snapshots
        d.skip()?; // non-myopic
        d.skip()?; // pulsing rewards
        d.skip()?; // pool distribution
        d.skip()?; // stashed

        Ok(DecodedLedgerState {
            utxos,
            pools,
            accounts,
            dreps,
            cc_members,
            cc_state,
            proposals,
            roots,
            pparams_hash,
            dormant_epochs,
            treasury,
            constitution,
        })
    }

    fn decode_segregated_parameters(
        dir: &Path,
        hash: &cbor::bytes::ByteSlice,
    ) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
        let pparams_file_path = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .find(|path| {
                path.file_name().map(|filename| filename.to_str() == Some(&hex::encode(hash.as_ref()))).unwrap_or(false)
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
        let decoded = decode_ledger_state(&mut decoder)?;

        let protocol_parameters = ProtocolParameters {
            protocol_version: PROTOCOL_VERSION_10,
            ..decode_segregated_parameters(pparams_dir, decoded.pparams_hash)?
        };

        let governance_activity =
            GovernanceActivity { consecutive_dormant_epochs: u64::from(decoded.dormant_epochs) as u32 };

        // NOTE:  DRep registration pointer fabrication
        //
        // a NewEpochState records no DRep registration pointer, so callers stamp a
        // synthesized `registered_at`. Any rule that orders against it,
        // e.g. "vote delegation must follow DRep registration", can't be meaningfully
        // checked on snapshot-seeded state; exercising that ordering needs an in-block
        // registration instead.
        let registered_at = CertificatePointer {
            transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
            certificate_index: 0,
        };

        let point = Point::Origin;

        let accounts: BTreeMap<StakeCredential, AccountState> = decoded
            .accounts
            .into_iter()
            // Pulsing reward updates aren't decoded, so the balance is the settled rewards only.
            .map(|(credential, account)| {
                (credential, snapshot::account_state(account, 0, &point, &protocol_parameters))
            })
            .collect();

        let dreps: BTreeMap<StakeCredential, DRepRegistration> = decoded
            .dreps
            .into_iter()
            .map(|(credential, state)| (credential, DRepRegistration::from_state(state, registered_at)))
            .collect();

        let committee = snapshot::committee_members(decoded.cc_state, &decoded.cc_members);

        let proposals = decoded
            .proposals
            .into_iter()
            .map(|st| {
                let action = ProposalSlim::from(&st.procedure.gov_action);
                (st.id, ProposalStateSlim { action, valid_until: st.expires_after })
            })
            .collect::<BTreeMap<_, _>>();

        let [root_params, root_hard_fork, root_cc, root_constitution] = decoded.roots;

        let proposals_roots = snapshot::proposals_roots(root_params, root_hard_fork, root_cc, root_constitution);

        let mut validation_context = DefaultValidationContext::new(
            decoded.utxos,
            decoded.pools,
            BTreeMap::new(),
            accounts,
            dreps,
            committee,
            proposals,
            proposals_roots,
            decoded.treasury,
        );

        let arena_pool = ArenaPool::new(1, 1_024_000);
        let global_parameters =
            NetworkName::Preprod.as_global_parameters().ok_or("missing global parameters for preprod")?;

        for (ix, event) in record.events.into_iter().enumerate() {
            let (tx_bytes, success, slot): (Bytes, bool, u64) = match event {
                TestVectorEvent::Transaction(tx, success, slot) => (tx, success, slot),
                TestVectorEvent::PassTick(..) | TestVectorEvent::PassEpoch(..) => continue,
            };
            let tx: Transaction = cbor::decode(tx_bytes.as_slice())?;

            let tx_witness_set: WitnessSet = tx.witnesses.clone();

            let tx_auxiliary_data = tx.auxiliary_data.as_ref();

            let pointer = TransactionPointer {
                slot: slot.into(),
                // Using the loop index here is conterintuitive but ensures that tx pointers will be distinct even if
                // the slots are the same. ultimately the pointers are made up since we do not have real blocks
                transaction_index: ix,
            };

            // NOTE: Transaction Size Calculation:
            //
            // As noted in block.rs, the transaction size is calcualted from an encoded 3-element list, not including the is_valid byte.
            // We don't have that here since the fixtures encode individual transactions, instead of blocks.
            // While the exact bytes aren't the same (the header should be 0x83 instead of 0x84), the is_valid boolean is exactly one byte.
            // So, by subtracting one, we get the expected value.
            let tx_size = (tx_bytes.len() - 1) as u64;

            // Run the transaction against the imported ledger state
            let result = transaction::phase_one::execute(
                &mut validation_context,
                &arena_pool,
                NetworkName::Preprod,
                &protocol_parameters,
                era_history,
                governance_activity,
                decoded.constitution.guardrail_script,
                pointer,
                tx.is_expected_valid,
                tx.body.clone(),
                &tx_witness_set,
                tx_auxiliary_data,
                tx_size,
            )
            .map_err(|e| e.to_string())
            .and_then(|_consumed_inputs| {
                transaction::phase_two::execute(
                    &mut validation_context,
                    &arena_pool,
                    &protocol_parameters,
                    era_history,
                    global_parameters,
                    pointer,
                    tx.is_expected_valid,
                    &tx.body,
                    &tx_witness_set,
                )
                .map_err(|e| e.to_string())
            });

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
