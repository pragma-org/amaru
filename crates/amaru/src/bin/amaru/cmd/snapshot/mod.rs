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

use clap::Subcommand;

pub(crate) mod create;
pub(crate) mod publish;
pub(crate) mod reindex;

#[derive(Debug, Subcommand)]
pub(crate) enum SnapshotCommand {
    /// Create the three consecutive epoch snapshots needed for bootstrap.
    Create(create::Args),

    /// Upload bootstrap snapshots to S3 and update the network index.
    Publish(publish::Args),

    /// Rebuild the snapshot index from archives stored in S3.
    Reindex(reindex::Args),
}
