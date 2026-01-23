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

use amaru_kernel::{MintedBlock, to_cbor};

use super::InvalidBlockDetails;

/// This validation checks that the purported block body size matches the actual block body size.
/// The validation of the bounds happens in the networking layer
pub fn block_body_size_valid(block: &MintedBlock<'_>) -> Result<(), InvalidBlockDetails> {
    let block_header = &block.header.header_body;
    let bh_size = block_header.block_body_size;
    let actual_block_size = calculate_block_body_size(block);

    if bh_size != actual_block_size {
        Err(InvalidBlockDetails::BlockSizeMismatch {
            supplied: bh_size,
            actual: actual_block_size,
        })
    } else {
        Ok(())
    }
}

// FIXME: Do not re-serialize block here, but rely on the original bytes.
fn calculate_block_body_size(block: &MintedBlock<'_>) -> u64 {
    let cbor_array_overhead = |n: u64| match n {
        n if n < 24 => 1,
        n if n < 256 => 2,
        n if n < 65536 => 3,
        n if n < 4294967296 => 5,
        _ => 9,
    };

    let (mut size_of_transactions, count_transactions) = block
        .transaction_bodies
        .iter()
        .fold((0, 0), |(acc, n), body| (acc + body.len(), n + 1));

    size_of_transactions += cbor_array_overhead(count_transactions);

    size_of_transactions
        + (to_cbor(&block.transaction_witness_sets).len() as u64)
        + (to_cbor(&block.auxiliary_data_set).len() as u64)
        + (to_cbor(&block.invalid_transactions).len() as u64)
}

#[cfg(test)]
mod tests {
    use crate::rules::block::InvalidBlockDetails;
    use amaru_kernel::{MintedBlock, include_cbor};
    use test_case::test_case;

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
        super::block_body_size_valid(&block)
    }
}
