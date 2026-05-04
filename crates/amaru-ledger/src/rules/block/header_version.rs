// Copyright 2026 PRAGMA
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

use amaru_kernel::{Block, ProtocolParameters};

use super::InvalidBlockDetails;

pub fn block_header_version_valid(
    block: &Block,
    protocol_params: &ProtocolParameters,
) -> Result<(), InvalidBlockDetails> {
    let header_major = block.header.header_body.protocol_version.0;
    let max_major = protocol_params.protocol_version.0 + 1;
    if header_major > max_major {
        return Err(InvalidBlockDetails::HeaderProtVerTooHigh { header_major, max_major });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Block, ProtocolParameters, include_cbor};
    use test_case::test_case;

    use crate::rules::block::InvalidBlockDetails;

    macro_rules! fixture {
        ($number:literal) => {
            (
                include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")),
                amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($number:literal, $pp:expr) => {
            (include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")), $pp)
        };
    }

    #[test_case(fixture!("2667657"); "valid")]
    #[test_case(fixture!("2667657", ProtocolParameters {
        protocol_version: (0, 0),
        ..amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()
    }) => matches Err(InvalidBlockDetails::HeaderProtVerTooHigh { header_major: 9, max_major: 1 }); "header version too high")]
    fn test_header_version((block, pp): (Block, ProtocolParameters)) -> Result<(), InvalidBlockDetails> {
        super::block_header_version_valid(&block, &pp)
    }
}
