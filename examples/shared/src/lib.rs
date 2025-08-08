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
    cbor, from_cbor,
    network::NetworkName,
    protocol_parameters::{self, GlobalParameters},
    to_cbor, Bytes, EraHistory, Hash, Hasher, MemoizedTransactionOutput, MintedBlock, Network,
    Point, PostAlonzoTransactionOutput, ProtocolVersion, TransactionInput, TransactionOutput,
    Value, PROTOCOL_VERSION_9,
};
use amaru_ledger::{
    context,
    rules::{self, block::BlockValidation},
    state::{State, VolatileState},
    store::{Store, TransactionalContext},
};
use amaru_stores::in_memory::MemoryStore;
use std::collections::BTreeMap;

type BlockWrapper<'b> = (u16, MintedBlock<'b>);

pub const RAW_BLOCK_CONWAY_1: &str = include_str!("../assets/conway1.block");
pub const RAW_BLOCK_CONWAY_3: &str = include_str!("../assets/conway3.block");

pub fn forward_ledger(raw_block: &str) {
    let bytes = hex::decode(raw_block).unwrap();

    let network = NetworkName::Preprod;
    let era_history: &EraHistory = network.into();

    let (_hash, block): BlockWrapper = cbor::decode(&bytes).unwrap();

    let global_parameters: &GlobalParameters = network.into();
    let protocol_version = &PROTOCOL_VERSION_9;

    let store = MemoryStore::new(era_history.clone(), *protocol_version);
    let historical_store = MemoryStore::new(era_history.clone(), *protocol_version);

    hydrate_initial_protocol_parameters(&store, &network, protocol_version);

    let mut state = State::new(
        store,
        historical_store,
        network,
        era_history.clone(),
        global_parameters.clone(),
    )
    .unwrap();

    let point = Point::Specific(
        block.header.header_body.slot,
        Hasher::<256>::hash(block.header.raw_cbor()).to_vec(),
    );

    let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);

    fn create_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()),
            index,
        }
    }

    fn create_output(address: &str) -> MemoizedTransactionOutput {
        let output = TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(hex::decode(address).unwrap()),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        });

        from_cbor(&to_cbor(&output)).unwrap()
    }

    let inputs = BTreeMap::from([
        (
            create_input(
                "2e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf4",
                0,
            ),
            create_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
        ),
        (
            create_input(
                "d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb",
                0,
            ),
            create_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
        ),
    ]);

    let mut context = context::DefaultValidationContext::new(inputs);
    if let BlockValidation::Invalid(_slot, _id, _err) = rules::validate_block(
        &mut context,
        &Network::from(network),
        state.protocol_parameters(),
        &block,
    ) {
        panic!("Failed to validate block")
    };

    let volatile_state: VolatileState = context.into();

    state
        .forward(volatile_state.anchor(&point, issuer))
        .unwrap()
}

pub fn hydrate_initial_protocol_parameters(
    store: &impl Store,
    network: &NetworkName,
    protocol_version: &ProtocolVersion,
) {
    let transaction = store.create_transaction();

    let params = match network {
        NetworkName::Preprod => &protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS,
        NetworkName::Preview => &protocol_parameters::PREVIEW_INITIAL_PROTOCOL_PARAMETERS,
        NetworkName::Mainnet | NetworkName::Testnet(..) => unimplemented!(),
    };

    transaction
        .set_protocol_parameters(params)
        .unwrap_or_else(|e| panic!("unable to set initial protocol parameters: {e}"));

    transaction
        .set_protocol_version(protocol_version)
        .unwrap_or_else(|e| panic!("unable to set initial protocol version: {e}"));

    transaction
        .commit()
        .unwrap_or_else(|e| panic!("unable to set initial ledger state: {e}"));
}
