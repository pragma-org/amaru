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

use amaru::lifecycle::Runnable;

pub mod create;
pub mod period;

/// Manage hot KES signing keys.
#[derive(Debug, clap::Subcommand)]
pub enum KesCommand {
    Create(create::CreateArgs),
    Period(period::PeriodArgs),
}

impl KesCommand {
    pub fn into_runnable(self) -> Runnable {
        match self {
            Self::Create(cmd) => cmd.into_runnable(),
            Self::Period(cmd) => cmd.into_runnable(),
        }
    }
}
