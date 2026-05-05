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

use crate::{MemoizedScript, PlutusVersion};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ScriptKind {
    Native,
    PlutusV1,
    PlutusV2,
    PlutusV3,
}

impl ScriptKind {
    pub fn is_native_script(&self) -> bool {
        matches!(self, Self::Native)
    }
}

impl From<PlutusVersion> for ScriptKind {
    fn from(version: PlutusVersion) -> Self {
        match version {
            PlutusVersion::V1 => ScriptKind::PlutusV1,
            PlutusVersion::V2 => ScriptKind::PlutusV2,
            PlutusVersion::V3 => ScriptKind::PlutusV3,
        }
    }
}

impl From<&MemoizedScript> for ScriptKind {
    fn from(value: &MemoizedScript) -> Self {
        match value {
            MemoizedScript::NativeScript(..) => ScriptKind::Native,
            MemoizedScript::PlutusV1Script(..) => ScriptKind::PlutusV1,
            MemoizedScript::PlutusV2Script(..) => ScriptKind::PlutusV2,
            MemoizedScript::PlutusV3Script(..) => ScriptKind::PlutusV3,
        }
    }
}
