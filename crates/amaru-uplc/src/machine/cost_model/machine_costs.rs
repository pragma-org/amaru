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
    pub fn new(cost_map: &HashMap<ParamName, i64>) -> Self {
        use ParamName::*;

        let param = |name: ParamName| cost_map.get(&name).copied().unwrap_or(i64::MAX);

        Self {
            startup: ExBudget { mem: param(CekStartupMem), cpu: param(CekStartupCpu) },
            constant: ExBudget { mem: param(CekConstMem), cpu: param(CekConstCpu) },
            var: ExBudget { mem: param(CekVarMem), cpu: param(CekVarCpu) },
            lambda: ExBudget { mem: param(CekLamMem), cpu: param(CekLamCpu) },
            apply: ExBudget { mem: param(CekApplyMem), cpu: param(CekApplyCpu) },
            delay: ExBudget { mem: param(CekDelayMem), cpu: param(CekDelayCpu) },
            force: ExBudget { mem: param(CekForceMem), cpu: param(CekForceCpu) },
            builtin: ExBudget { mem: param(CekBuiltinMem), cpu: param(CekBuiltinCpu) },
            constr: ExBudget { mem: param(CekConstrMem), cpu: param(CekConstrCpu) },
            case: ExBudget { mem: param(CekCaseMem), cpu: param(CekCaseCpu) },
        }
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
