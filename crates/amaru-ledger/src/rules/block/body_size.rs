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

use amaru_kernel::{to_cbor, MintedBlock};

use super::{BlockValidation, InvalidBlockDetails};

/// This validation checks that the purported block body size matches the actual block body size.
/// The validation of the bounds happens in the networking layer
pub fn block_body_size_valid(block: &MintedBlock<'_>) -> BlockValidation {
    let block_header = &block.header.header_body;
    let bh_size = block_header.block_body_size as usize;
    let actual_block_size = calculate_block_body_size(block);

    if bh_size != actual_block_size {
        BlockValidation::Invalid(InvalidBlockDetails::BlockSizeMismatch {
            supplied: bh_size,
            actual: actual_block_size,
        })
    } else {
        BlockValidation::Valid
    }
}

// FIXME: Do not re-serialize block here, but rely on the original bytes.
fn calculate_block_body_size(block: &MintedBlock<'_>) -> usize {
    let tx_bodies_raw = to_cbor(&block.transaction_bodies);
    let tx_witness_sets_raw = to_cbor(&block.transaction_witness_sets);
    let auxiliary_data_raw = to_cbor(&block.auxiliary_data_set);
    let invalid_transactions_raw = to_cbor(&block.invalid_transactions);

    tx_bodies_raw.len()
        + tx_witness_sets_raw.len()
        + auxiliary_data_raw.len()
        + invalid_transactions_raw.len()
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{include_cbor, MintedBlock};
    use test_case::test_case;

    use crate::rules::block::InvalidBlockDetails;

    macro_rules! fixture {
        ($number:literal) => {
            include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor"))
        };
        ($number:literal, $variant:literal) => {
            include_cbor!(concat!("blocks/preprod/", $number, "/", $variant, ".cbor"))
        };
    }

    #[test_case(fixture!("2667660"); "valid")]
    #[test_case(fixture!("2667660", "invalid_block_body_size") => 
        matches Err(InvalidBlockDetails::BlockSizeMismatch {supplied, actual})
            if supplied == 0 && actual == 3411;
    "block body size mismatch")]
    fn test_block_size(block: MintedBlock<'_>) -> Result<(), InvalidBlockDetails> {
        super::block_body_size_valid(&block).into()
    }
}
