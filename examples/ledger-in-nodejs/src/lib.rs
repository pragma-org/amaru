use amaru_examples_shared::{RAW_BLOCK_CONWAY_1, forward_ledger};

#[no_mangle]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn ledger() {
    forward_ledger(RAW_BLOCK_CONWAY_1);
}
