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

use amaru_kernel::{protocol_parameters::ProtocolParameters, sum_ex_units, ExUnits};

use super::{BlockValidation, InvalidBlockDetails};

pub fn block_ex_units_valid(
    ex_units: Vec<ExUnits>,
    protocol_parameters: &ProtocolParameters,
) -> BlockValidation {
    // TODO: rewrite this to use iterators defined on `Redeemers` and `MaybeIndefArray`, ideally

    let pp_max_ex_units = protocol_parameters.max_block_ex_units;
    let ex_units = ex_units
        .into_iter()
        .fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units);

    if ex_units.mem <= pp_max_ex_units.mem && ex_units.steps <= pp_max_ex_units.steps {
        BlockValidation::Valid
    } else {
        BlockValidation::Invalid(InvalidBlockDetails::TooManyExUnits {
            provided: ex_units,
            max: ExUnits {
                mem: pp_max_ex_units.mem,
                steps: pp_max_ex_units.steps,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::block::InvalidBlockDetails;

    use amaru_kernel::{
        include_cbor, protocol_parameters::ProtocolParameters, ExUnits, HasExUnits, MintedBlock,
    };
    use test_case::test_case;

    macro_rules! fixture {
        ($number:literal) => {
            (
                include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")),
                ProtocolParameters::default(),
            )
        };
        ($number:literal, $pp:expr) => {
            (
                include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")),
                $pp,
            )
        };
    }

    #[test_case(fixture!("2667657"); "valid ex units")]
    #[test_case(fixture!("2667657", ProtocolParameters {
        max_block_ex_units: ExUnits {
            mem: 0,
            steps: 0
        },
        ..Default::default()
    }) => matches Err(InvalidBlockDetails::TooManyExUnits{provided, max: _})
    if provided == ExUnits {mem: 1267029, steps: 289959162}; "invalid ex units")]
    fn test_ex_units(
        (block, protocol_parameters): (MintedBlock<'_>, ProtocolParameters),
    ) -> Result<(), InvalidBlockDetails> {
        super::block_ex_units_valid(block.ex_units(), &protocol_parameters).into()
    }
}
