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

#[allow(clippy::disallowed_types)]
use std::collections::HashMap;

use amaru_kernel::PlutusVersion;

use crate::machine::{
    ExBudget,
    cost_model::{ParamName, StepKind},
};

#[derive(Debug, PartialEq)]
pub struct MachineCosts {
    pub startup: ExBudget,
    pub constant: ExBudget,
    pub var: ExBudget,
    pub lambda: ExBudget,
    pub delay: ExBudget,
    pub force: ExBudget,
    pub apply: ExBudget,
    pub constr: ExBudget,
    pub case: ExBudget,
    pub builtin: ExBudget,
}

impl Default for MachineCosts {
    fn default() -> Self {
        Self {
            startup: Self::default_startup_cost(),
            constant: Self::default_machine_cost(),
            var: Self::default_machine_cost(),
            lambda: Self::default_machine_cost(),
            apply: Self::default_machine_cost(),
            delay: Self::default_machine_cost(),
            force: Self::default_machine_cost(),
            builtin: Self::default_machine_cost(),
            constr: Self::default_machine_cost(),
            case: Self::default_machine_cost(),
        }
    }
}

impl MachineCosts {
    #[allow(clippy::disallowed_types)]
    pub fn new(cost_map: &HashMap<ParamName, i64>, plutus_version: PlutusVersion) -> Result<Self, ParamName> {
        use ParamName::*;

        let always = |name: ParamName| cost_map.get(&name).copied().ok_or(name);

        let if_v3: Box<dyn Fn(ParamName) -> Result<i64, ParamName>> = if plutus_version >= PlutusVersion::V3 {
            Box::new(always)
        } else {
            Box::new(|_name: ParamName| Ok(i64::MAX))
        };

        Ok(Self {
            startup: ExBudget { mem: always(CekStartupMem)?, cpu: always(CekStartupCpu)? },
            constant: ExBudget { mem: always(CekConstMem)?, cpu: always(CekConstCpu)? },
            var: ExBudget { mem: always(CekVarMem)?, cpu: always(CekVarCpu)? },
            lambda: ExBudget { mem: always(CekLamMem)?, cpu: always(CekLamCpu)? },
            apply: ExBudget { mem: always(CekApplyMem)?, cpu: always(CekApplyCpu)? },
            delay: ExBudget { mem: always(CekDelayMem)?, cpu: always(CekDelayCpu)? },
            force: ExBudget { mem: always(CekForceMem)?, cpu: always(CekForceCpu)? },
            builtin: ExBudget { mem: always(CekBuiltinMem)?, cpu: always(CekBuiltinCpu)? },
            constr: ExBudget { mem: if_v3(CekConstrMem)?, cpu: if_v3(CekConstrCpu)? },
            case: ExBudget { mem: if_v3(CekCaseMem)?, cpu: if_v3(CekCaseCpu)? },
        })
    }

    pub fn step(&self, step_kind: StepKind) -> ExBudget {
        match step_kind {
            StepKind::Constant => self.constant,
            StepKind::Var => self.var,
            StepKind::Lambda => self.lambda,
            StepKind::Apply => self.apply,
            StepKind::Delay => self.delay,
            StepKind::Force => self.force,
            StepKind::Builtin => self.builtin,
            StepKind::Constr => self.constr,
            StepKind::Case => self.case,
        }
    }

    pub fn default_startup_cost() -> ExBudget {
        ExBudget { mem: 100, cpu: 100 }
    }

    pub fn default_machine_cost() -> ExBudget {
        ExBudget { mem: 100, cpu: 16000 }
    }
}
