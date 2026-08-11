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

mod card;
mod config_sections;
mod epoch_progress;
mod gauge_card;
mod logs;
mod memory_card;
mod peers;
mod proposals;

pub(super) use self::{
    card::render_card,
    config_sections::render_section_groups,
    epoch_progress::render_epoch_progress,
    gauge_card::render_gauge_card,
    logs::render_logs,
    memory_card::{render_process_memory_card, render_rss_memory_card},
    peers::render_peers_table,
    proposals::render_proposals_table,
};
