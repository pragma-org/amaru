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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::text::{Line, Span};

use super::theme::{emphasis_white, label_style};

pub(super) fn aligned_pair_lines(entries: Vec<(&'static str, String)>) -> Vec<Line<'static>> {
    let label_width = entries.iter().map(|(label, _)| label.len()).max().unwrap_or_default();

    entries
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<label_width$}    "), label_style()),
                Span::styled(value, emphasis_white()),
            ])
        })
        .collect()
}

pub(super) fn format_count(value: impl TryInto<u64>) -> String {
    let value = value.try_into().ok().unwrap_or_default();
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(ch);
    }

    formatted
}

pub(super) fn format_density(density: f64, active_slot_coeff_inverse: u64) -> String {
    format!("{:.2}%", density * active_slot_coeff_inverse as f64 * 100.0)
}

pub(super) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

pub(super) fn format_log_wall_time(wall_time: SystemTime) -> String {
    let seconds = wall_time.duration_since(UNIX_EPOCH).map(|duration| duration.as_secs() % 86_400).unwrap_or_default();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}")
}

pub(super) fn format_lovelace(value: u64) -> String {
    let ada = value / 1_000_000;
    let lovelace = value % 1_000_000;
    format!("₳{}.{lovelace:06}", format_count(ada))
}

pub(super) fn format_ratio(left: u64, right: u64) -> String {
    format!("{} / {}", format_count(left), format_count(right))
}

pub(super) fn format_slot_ratio(slot: u64, target: Option<u64>) -> String {
    target.map(|target| format_ratio(slot, target)).unwrap_or_else(|| format_count(slot))
}
