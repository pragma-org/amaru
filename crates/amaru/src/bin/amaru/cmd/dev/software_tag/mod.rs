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
use clap::Subcommand;

pub(crate) mod decode;

#[derive(Debug, Subcommand)]
pub(crate) enum SoftwareTagCommand {
    /// Decode the CIP-0203 block producer tag carried in a header's protocol version.
    Decode(decode::Args),

    /// Print the tag this binary writes into the headers it forges.
    Current,
}

impl SoftwareTagCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Decode(args) => decode::runnable(args),
            Self::Current => decode::runnable(decode::Args::current()),
        }
    }
}
