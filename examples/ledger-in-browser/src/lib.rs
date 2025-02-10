use amaru_kernel::{cbor, Hasher, MintedBlock, Point};
use amaru_ledger::state::State;
use std::sync::{Arc, Mutex};
use store::MemoryStore;
use tracing::trace_span;

mod store;

type BlockWrapper<'b> = (u16, MintedBlock<'b>);

#[no_mangle]
pub unsafe extern "C" fn ledger() -> () {
    let raw_block = include_str!("../../assets/conway1.block");
    let bytes = hex::decode(raw_block).unwrap();
    let (_, block): BlockWrapper = cbor::decode(&bytes).unwrap();
    let mut state = State::new(Arc::new(Mutex::new(MemoryStore {})));
    let point = Point::Specific(
        block.header.header_body.slot,
        Hasher::<256>::hash(block.header.raw_cbor()).to_vec(),
    );
    state.forward(&trace_span!("ledger"), &point, block).unwrap()
}
