use amaru_kernel::{
    cbor, Bytes, Hash, Hasher, MintedBlock, Point,
    PostAlonzoTransactionOutput, TransactionInput, TransactionOutput, Value, EraHistory,
    network::NetworkName, protocol_parameters::{GlobalParameters, ProtocolParameters},
};
use amaru_ledger::{
    context,
    rules::{self, block::BlockValidation},
    state::{State, VolatileState, stake_distributions},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use store::MemoryStore;

mod store;

type BlockWrapper<'b> = (u16, MintedBlock<'b>);

pub const RAW_BLOCK_CONWAY_1: &str = include_str!("../assets/conway1.block");
pub const RAW_BLOCK_CONWAY_3: &str = include_str!("../assets/conway3.block");

pub fn forward_ledger(raw_block: &str) {
    let bytes = hex::decode(raw_block).unwrap();

    let (_hash, block): BlockWrapper = cbor::decode(&bytes).unwrap();
    let era_history : &EraHistory = NetworkName::Preprod.into();

    let global_parameters = GlobalParameters::default();
    let protocol_parameters = ProtocolParameters::default();
    let store = MemoryStore {};
    let latest_epoch = store.most_recent_snapshot();
    let stake_distributions =
        stake_distributions(latest_epoch, &store, &store, era_history).unwrap();
    let mut state = State::new(Arc::new(Mutex::new(store)), MemoryStore {}, era_history, &global_parameters, stake_distributions);

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

    fn create_output(address: &str) -> TransactionOutput {
        TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(hex::decode(address).unwrap()),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        })
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
    if let BlockValidation::Invalid(_err) = rules::validate_block(
        &mut context,
        &ProtocolParameters::default(),
        &block,
    ) {
        panic!("Failed to validate block")
    };

    let volatile_state: VolatileState = context.into();

    state.forward(&global_parameters, &protocol_parameters, volatile_state.anchor(&point, issuer)).unwrap()
}
