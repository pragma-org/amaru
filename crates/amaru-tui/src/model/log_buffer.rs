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

use std::{collections::VecDeque, rc::Rc};

use regex::Regex;
use tracing::Level;

use super::{LevelFilter, TargetFilter};
use crate::{config::DEFAULT_LOG_RETENTION_BYTES, events::TelemetryRecord};

/// Retention class for a stored log line. Newer classes keep more detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionTier {
    /// Newest window: TRACE/DEBUG/INFO/WARN/ERROR.
    DebugAndUp,
    /// Next-oldest: INFO/WARN/ERROR.
    InfoAndUp,
    /// Next-oldest: WARN/ERROR.
    WarnAndUp,
    /// Oldest window: ERROR only.
    Error,
}

impl RetentionTier {
    pub const ALL_NEWEST_FIRST: [Self; 4] = [Self::DebugAndUp, Self::InfoAndUp, Self::WarnAndUp, Self::Error];
    pub const ALL_OLDEST_FIRST: [Self; 4] = [Self::Error, Self::WarnAndUp, Self::InfoAndUp, Self::DebugAndUp];

    pub fn index(self) -> usize {
        match self {
            Self::DebugAndUp => 0,
            Self::InfoAndUp => 1,
            Self::WarnAndUp => 2,
            Self::Error => 3,
        }
    }

    pub fn share_percent(self) -> usize {
        match self {
            Self::DebugAndUp => 70,
            Self::InfoAndUp | Self::WarnAndUp | Self::Error => 10,
        }
    }

    pub fn allows(self, level: Level) -> bool {
        match self {
            Self::DebugAndUp => true,
            Self::InfoAndUp => matches!(level, Level::INFO | Level::WARN | Level::ERROR),
            Self::WarnAndUp => matches!(level, Level::WARN | Level::ERROR),
            Self::Error => level == Level::ERROR,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DebugAndUp => "debug+",
            Self::InfoAndUp => "info+",
            Self::WarnAndUp => "warn+",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub enum LogViewItem {
    Record { record: Rc<TelemetryRecord>, tier: RetentionTier },
    TierBoundary { tier: RetentionTier },
}

impl LogViewItem {
    pub fn record(&self) -> Option<&Rc<TelemetryRecord>> {
        match self {
            Self::Record { record, .. } => Some(record),
            Self::TierBoundary { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct StoredLog {
    record: Rc<TelemetryRecord>,
    bytes: usize,
}

#[derive(Debug)]
pub struct LogBuffer {
    /// log entries per tier, newest tier first, oldest message within tier first
    tiers: [VecDeque<StoredLog>; 4],
    tier_bytes: [usize; 4],
    budgets: [usize; 4],
    /// matching log entries, oldest first
    view: Vec<LogViewItem>,
    dirty: bool,
    view_level: LevelFilter,
    view_target: TargetFilter,
    view_pattern: String,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_LOG_RETENTION_BYTES)
    }
}

impl LogBuffer {
    pub fn new(budget: usize) -> Self {
        Self {
            tiers: std::array::from_fn(|_| VecDeque::new()),
            tier_bytes: [0; 4],
            budgets: tier_budgets(budget),
            view: Vec::new(),
            dirty: false,
            view_level: LevelFilter::Info,
            view_target: TargetFilter::All,
            view_pattern: String::new(),
        }
    }

    pub fn view(&self) -> &[LogViewItem] {
        &self.view
    }

    /// Used bytes and allowance for each retention tier, newest first.
    pub fn occupancy(&self) -> [(RetentionTier, usize, usize); 4] {
        RetentionTier::ALL_NEWEST_FIRST.map(|tier| {
            let index = tier.index();
            (tier, self.tier_bytes[index], self.budgets[index])
        })
    }

    pub fn push(&mut self, record: TelemetryRecord) {
        let bytes = estimated_bytes(&record);
        if bytes == 0 {
            return;
        }

        self.tiers[RetentionTier::DebugAndUp.index()].push_back(StoredLog { record: Rc::new(record), bytes });
        self.tier_bytes[RetentionTier::DebugAndUp.index()] += bytes;
        self.spill();
        self.dirty = true;
    }

    pub fn sync(
        &mut self,
        level_filter: LevelFilter,
        target_filter: TargetFilter,
        pattern: &str,
        text_filter: Option<&Regex>,
    ) {
        if !self.dirty
            && self.view_level == level_filter
            && self.view_target == target_filter
            && self.view_pattern == pattern
        {
            return;
        }

        self.rebuild_view(level_filter, target_filter, pattern, text_filter);
    }

    fn rebuild_view(
        &mut self,
        level_filter: LevelFilter,
        target_filter: TargetFilter,
        pattern: &str,
        text_filter: Option<&Regex>,
    ) {
        self.view.clear();

        for tier in RetentionTier::ALL_OLDEST_FIRST {
            let start = self.view.len();
            for stored in &self.tiers[tier.index()] {
                if matches_filters(&stored.record, level_filter, target_filter, text_filter) {
                    self.view.push(LogViewItem::Record { record: Rc::clone(&stored.record), tier });
                }
            }

            let has_visible = self.view.len() > start;
            if has_visible && tier != RetentionTier::DebugAndUp {
                self.view.push(LogViewItem::TierBoundary { tier });
            }
        }

        self.dirty = false;
        self.view_level = level_filter;
        self.view_target = target_filter;
        self.view_pattern = pattern.to_string();
    }

    fn spill(&mut self) {
        for tier in RetentionTier::ALL_NEWEST_FIRST {
            let index = tier.index();
            while self.tier_bytes[index] > self.budgets[index] {
                let Some(oldest) = self.tiers[index].pop_front() else {
                    break;
                };
                self.tier_bytes[index] = self.tier_bytes[index].saturating_sub(oldest.bytes);

                let Some(next) = RetentionTier::ALL_NEWEST_FIRST.get(index + 1).copied() else {
                    continue;
                };
                if next.allows(oldest.record.level) {
                    self.tier_bytes[next.index()] += oldest.bytes;
                    self.tiers[next.index()].push_back(oldest);
                }
            }
        }
    }

    #[cfg(test)]
    fn tier_len(&self, tier: RetentionTier) -> usize {
        self.tiers[tier.index()].len()
    }

    #[cfg(test)]
    fn tier_levels(&self, tier: RetentionTier) -> Vec<Level> {
        self.tiers[tier.index()].iter().map(|stored| stored.record.level).collect()
    }
}

fn tier_budgets(total: usize) -> [usize; 4] {
    let mut budgets = [0; 4];
    for tier in RetentionTier::ALL_NEWEST_FIRST {
        budgets[tier.index()] = total.saturating_mul(tier.share_percent()) / 100;
    }
    let assigned: usize = budgets.iter().sum();
    budgets[RetentionTier::DebugAndUp.index()] += total.saturating_sub(assigned);
    budgets
}

/// Estimate a rough approximation of the heap size needed for retaining this record.
pub(crate) fn estimated_bytes(record: &TelemetryRecord) -> usize {
    const BASE: usize = 96;
    let mut bytes = BASE + record.target.len() + record.name.len();
    bytes += record.span_name.as_ref().map(String::len).unwrap_or(0);
    bytes += record.parents.iter().map(|parent| parent.len() + 8).sum::<usize>();
    bytes += record.fields.iter().map(|(name, value)| name.len() + 16 + field_bytes(value)).sum::<usize>();
    bytes.max(64)
}

fn field_bytes(value: &crate::events::FieldValue) -> usize {
    match value {
        crate::events::FieldValue::Bool(_)
        | crate::events::FieldValue::I64(_)
        | crate::events::FieldValue::U64(_)
        | crate::events::FieldValue::F64(_) => 8,
        crate::events::FieldValue::String(value) => value.len(),
    }
}

fn matches_filters(
    record: &TelemetryRecord,
    level_filter: LevelFilter,
    target_filter: TargetFilter,
    text_filter: Option<&Regex>,
) -> bool {
    if !level_filter.allows(record.level) || !target_filter.allows(&record.target) {
        return false;
    }

    match text_filter {
        Some(regex) => regex.is_match(&record.plain_text()),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, time::Instant};

    use tracing::Level;

    use super::*;
    use crate::events::FieldValue;

    fn record(level: Level, name: &str, pad: usize) -> TelemetryRecord {
        let mut fields = BTreeMap::new();
        if pad > 0 {
            fields.insert("pad".into(), FieldValue::String("x".repeat(pad)));
        }
        TelemetryRecord {
            level,
            target: "amaru::ledger".into(),
            name: name.into(),
            at: Instant::now(),
            wall_time: std::time::SystemTime::UNIX_EPOCH,
            fields,
            parents: Vec::new(),
            span_name: None,
            id: None,
            parent_id: None,
        }
    }

    #[test]
    fn newest_tier_keeps_debug_until_spill() {
        let mut buffer = LogBuffer::new(1_000);
        buffer.push(record(Level::DEBUG, "recent", 200));
        assert_eq!(buffer.tier_len(RetentionTier::DebugAndUp), 1);
        assert_eq!(buffer.tier_len(RetentionTier::InfoAndUp), 0);
    }

    #[test]
    fn spilling_drops_debug_and_moves_info_and_error() {
        let mut buffer = LogBuffer::new(4_000);
        buffer.push(record(Level::ERROR, "old-error", 80));
        buffer.push(record(Level::INFO, "old-info", 80));
        buffer.push(record(Level::DEBUG, "old-debug", 80));
        for index in 0..20 {
            buffer.push(record(Level::DEBUG, &format!("new-debug-{index}"), 80));
        }

        assert!(!buffer.tier_levels(RetentionTier::DebugAndUp).contains(&Level::ERROR));
        assert!(
            buffer.tier_levels(RetentionTier::InfoAndUp).contains(&Level::ERROR)
                || buffer.tier_levels(RetentionTier::WarnAndUp).contains(&Level::ERROR)
                || buffer.tier_levels(RetentionTier::Error).contains(&Level::ERROR)
        );
        assert!(
            buffer.tier_levels(RetentionTier::InfoAndUp).contains(&Level::INFO)
                || buffer.tier_levels(RetentionTier::WarnAndUp).contains(&Level::INFO)
        );
        assert!(!buffer.tier_levels(RetentionTier::InfoAndUp).contains(&Level::DEBUG));
        assert!(!buffer.tier_levels(RetentionTier::WarnAndUp).contains(&Level::DEBUG));
        assert!(!buffer.tier_levels(RetentionTier::Error).contains(&Level::DEBUG));
    }

    #[test]
    fn view_inserts_boundary_after_older_tiers() {
        let mut buffer = LogBuffer::new(4_000);
        buffer.push(record(Level::ERROR, "old-error", 80));
        for index in 0..20 {
            buffer.push(record(Level::DEBUG, &format!("pad-{index}"), 80));
        }
        buffer.push(record(Level::ERROR, "new-error", 80));
        buffer.sync(LevelFilter::Error, TargetFilter::All, "", None);

        let boundaries: Vec<_> = buffer
            .view()
            .iter()
            .filter_map(|item| match item {
                LogViewItem::TierBoundary { tier } => Some(*tier),
                LogViewItem::Record { .. } => None,
            })
            .collect();
        assert!(!boundaries.is_empty());
        assert!(!boundaries.contains(&RetentionTier::DebugAndUp));
    }

    #[test]
    fn text_filter_matches_plain_log_line() {
        let mut buffer = LogBuffer::new(1_000);
        buffer.push(record(Level::INFO, "alpha-event", 0));
        buffer.push(record(Level::INFO, "beta-event", 0));
        let regex = Regex::new("alpha-event").unwrap();
        buffer.sync(LevelFilter::Info, TargetFilter::All, "alpha-event", Some(&regex));

        let names: Vec<_> =
            buffer.view().iter().filter_map(|item| item.record().map(|record| record.name.clone())).collect();
        assert_eq!(names, vec!["alpha-event".to_string()]);
    }
}
