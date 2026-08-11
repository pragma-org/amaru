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

use ratatui::{Frame, layout::Rect};

use super::gauge_card::render_gauge_card;
use crate::{events::SystemSample, model::InteractionMode, ui::format::format_count};

pub(in crate::ui) fn render_process_memory_card(
    frame: &mut Frame<'_>,
    area: Rect,
    sample: Option<&SystemSample>,
    mode: InteractionMode,
) {
    render_memory_metric_card(frame, area, "Memory", sample, mode, |sample| sample.process_memory_bytes);
}

pub(in crate::ui) fn render_rss_memory_card(
    frame: &mut Frame<'_>,
    area: Rect,
    sample: Option<&SystemSample>,
    mode: InteractionMode,
) {
    render_memory_metric_card(frame, area, "RSS", sample, mode, |sample| sample.rss_bytes);
}

fn render_memory_metric_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    sample: Option<&SystemSample>,
    mode: InteractionMode,
    current: impl Fn(&SystemSample) -> u64,
) {
    let (value, percent, ratio) = sample
        .map(|sample| {
            let current_bytes = current(sample);
            let total_bytes = sample.memory_total_bytes;
            (
                Some(format_mib(current_bytes)),
                Some(format!("{:.1}%", memory_ratio(current_bytes, total_bytes) * 100.0)),
                memory_ratio(current_bytes, total_bytes),
            )
        })
        .unwrap_or((None, None, 0.0));

    render_gauge_card(frame, area, title, value, ratio, percent, mode);
}

fn memory_ratio(current_bytes: u64, total_bytes: u64) -> f64 {
    if total_bytes == 0 { 0.0 } else { (current_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0) }
}

fn format_mib(bytes: u64) -> String {
    format!("{} MiB", format_count(bytes.div_ceil(1_048_576)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_ratio_keeps_bounds() {
        assert_eq!(memory_ratio(0, 1_000), 0.0);
        assert_eq!(memory_ratio(1_000, 1_000), 1.0);
        assert_eq!(memory_ratio(42, 300), 0.14);
    }

    #[test]
    fn formats_memory_value_in_mib() {
        assert_eq!(format_mib(512 * 1_048_576), "512 MiB");
        assert_eq!(format_mib(2_048 * 1_048_576), "2,048 MiB");
    }
}
