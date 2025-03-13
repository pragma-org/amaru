use amaru_kernel::{protocol_parameters::ProtocolParameters, sum_ex_units, ExUnits};

use crate::rules::RuleViolation;

pub fn block_ex_units_valid(
    ex_units: Vec<ExUnits>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), RuleViolation> {
    let pp_max_ex_units = protocol_parameters.max_block_ex_units;
    let ex_units = ex_units
        .into_iter()
        .fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units);

    if ex_units.mem <= pp_max_ex_units.mem && ex_units.steps <= pp_max_ex_units.steps {
        Ok(())
    } else {
        Err(RuleViolation::TooManyExUnitsBlock {
            provided: ex_units,
            max: ExUnits {
                mem: pp_max_ex_units.mem,
                steps: pp_max_ex_units.steps,
            },
        })
    }
}
