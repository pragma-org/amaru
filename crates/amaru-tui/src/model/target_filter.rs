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

use super::telemetry_event::{CONSENSUS_TARGET, LEDGER_TARGET, PROTOCOLS_TARGET};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFilter {
    All,
    Ledger,
    Consensus,
    Protocols,
    Other,
}

impl TargetFilter {
    pub const ALL: [Self; 5] = [Self::All, Self::Ledger, Self::Consensus, Self::Protocols, Self::Other];

    pub fn allows(self, target: &str) -> bool {
        match self {
            Self::All => true,
            Self::Ledger => target == LEDGER_TARGET,
            Self::Consensus => target == CONSENSUS_TARGET,
            Self::Protocols => target == PROTOCOLS_TARGET,
            Self::Other => !matches!(target, LEDGER_TARGET | CONSENSUS_TARGET | PROTOCOLS_TARGET),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Ledger => "ledger",
            Self::Consensus => "consensus",
            Self::Protocols => "protocols",
            Self::Other => "other",
        }
    }
}
