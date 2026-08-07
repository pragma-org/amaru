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

use std::rc::Rc;

use tracing::Level;

use super::{LevelFilter, TargetFilter};
use crate::{config::Config, events::TelemetryRecord};

#[derive(Debug, Default)]
pub struct LogBuffer {
    all: Vec<Rc<TelemetryRecord>>,
    filtered: Vec<Rc<TelemetryRecord>>,
    debug_count: usize,
    info_count: usize,
    warn_count: usize,
    error_count: usize,
}

impl LogBuffer {
    pub fn filtered(&self) -> &[Rc<TelemetryRecord>] {
        &self.filtered
    }

    pub fn push(
        &mut self,
        record: TelemetryRecord,
        config: &Config,
        level_filter: LevelFilter,
        target_filter: TargetFilter,
    ) {
        let level = record.level;
        let capacity = config.log_capacity_for(level);
        if capacity == 0 {
            return;
        }

        while self.count(level) >= capacity {
            self.evict_oldest(level);
        }

        let record = Rc::new(record);
        self.increment(level);
        if matches_filters(&record, level_filter, target_filter) {
            self.filtered.push(Rc::clone(&record));
        }
        self.all.push(record);
    }

    pub fn rebuild_filtered(&mut self, level_filter: LevelFilter, target_filter: TargetFilter) {
        self.filtered.clear();
        self.filtered
            .extend(self.all.iter().filter(|record| matches_filters(record, level_filter, target_filter)).cloned());
    }

    fn evict_oldest(&mut self, level: Level) {
        let Some(position) = self.all.iter().position(|record| record.level == level) else {
            return;
        };

        let removed = self.all.remove(position);
        self.decrement(level);

        if let Some(position) = self.filtered.iter().position(|record| Rc::ptr_eq(record, &removed)) {
            self.filtered.remove(position);
        }
    }

    fn count(&self, level: Level) -> usize {
        match level {
            Level::TRACE | Level::DEBUG => self.debug_count,
            Level::INFO => self.info_count,
            Level::WARN => self.warn_count,
            Level::ERROR => self.error_count,
        }
    }

    fn increment(&mut self, level: Level) {
        match level {
            Level::TRACE | Level::DEBUG => self.debug_count += 1,
            Level::INFO => self.info_count += 1,
            Level::WARN => self.warn_count += 1,
            Level::ERROR => self.error_count += 1,
        }
    }

    fn decrement(&mut self, level: Level) {
        match level {
            Level::TRACE | Level::DEBUG => self.debug_count = self.debug_count.saturating_sub(1),
            Level::INFO => self.info_count = self.info_count.saturating_sub(1),
            Level::WARN => self.warn_count = self.warn_count.saturating_sub(1),
            Level::ERROR => self.error_count = self.error_count.saturating_sub(1),
        }
    }
}

fn matches_filters(record: &TelemetryRecord, level_filter: LevelFilter, target_filter: TargetFilter) -> bool {
    level_filter.allows(record.level) && target_filter.allows(&record.target)
}
