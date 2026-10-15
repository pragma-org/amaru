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

#[cfg(all(not(target_family = "wasm"), not(target_arch = "riscv32")))]
const _: () = amaru_deps::AMARU_DEPS_USED;

mod download;
mod immutable;

pub use download::{
    MithrilDownloadError, MithrilDownloadObserver, MithrilDownloadProgress, MithrilDownloadReport,
    MithrilDownloadStage, download_from_mithril, download_from_mithril_for_range_with_observer,
};
pub use immutable::{
    ImmutableBlock, chunk_for_slot, first_missing_immutable_chunk, iter_immutable_blocks, last_immutable_point,
    read_blocks_after_point,
};
