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
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidExUnits {
    #[error("too many execution units in block: provided (mem: {}, steps: {}), max (mem: {}, steps: {})", provided.mem, provided.steps, max.mem, max.steps)]
    TooMany { provided: ExUnits, max: ExUnits },
}

pub fn block_ex_units_valid(
    ex_units: Vec<ExUnits>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidExUnits> {
    let pp_max_ex_units = protocol_parameters.max_block_ex_units;
    let ex_units = ex_units
        .into_iter()
        .fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units);

    if ex_units.mem <= pp_max_ex_units.mem && ex_units.steps <= pp_max_ex_units.steps {
        Ok(())
    } else {
        Err(InvalidExUnits::TooMany {
            provided: ex_units,
            max: ExUnits {
                mem: pp_max_ex_units.mem,
                steps: pp_max_ex_units.steps,
            },
        })
    }
}
