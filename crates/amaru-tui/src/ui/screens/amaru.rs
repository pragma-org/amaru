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

use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use super::super::{
    components::{
        render_card, render_gauge_card, render_peers_table, render_process_memory_card, render_rss_memory_card,
    },
    format::{
        aligned_pair_lines, format_count, format_density, format_duration, format_secs_frequency, format_slot_ratio,
    },
};
use crate::{events::SystemSample, model::Model, ui::Views};

pub(in crate::ui) fn render_amaru(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views, now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(peers_panel_height(model).max(mempool_panel_height())),
        ])
        .split(area);

    let charts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .split(layout[0]);
    let sample = latest_system_sample(model);
    render_process_memory_card(frame, charts[0], sample, model.interaction_mode);
    render_rss_memory_card(frame, charts[1], sample, model.interaction_mode);
    let cpu = cpu_gauge(sample);
    render_gauge_card(frame, charts[2], "CPU", cpu.value, cpu.ratio, cpu.percent, model.interaction_mode);
    let disk_read = disk_read_gauge(sample);
    render_gauge_card(
        frame,
        charts[3],
        "Disk Read",
        disk_read.value,
        disk_read.ratio,
        disk_read.percent,
        model.interaction_mode,
    );
    let disk_write = disk_write_gauge(sample);
    render_gauge_card(
        frame,
        charts[4],
        "Disk Write",
        disk_write.value,
        disk_write.ratio,
        disk_write.percent,
        model.interaction_mode,
    );

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1)])
        .split(layout[1]);

    render_card(
        frame,
        cards[0],
        "Node",
        aligned_pair_lines(vec![
            ("Network", model.startup.process.network.clone()),
            ("Platform", model.startup.process.target.clone()),
            ("PID", model.startup.process.pid.to_string()),
            ("Uptime", format_duration(now.duration_since(model.created_at))),
        ]),
        model.interaction_mode,
    );

    let block_rate = blocks_per_second(model);
    let transaction_rate = transactions_per_second(model);
    render_card(
        frame,
        cards[1],
        "Throughput",
        aligned_pair_lines(vec![
            ("Blocks", format_count(model.recent_blocks_count())),
            ("Blocks/s", format!("{block_rate:.2}")),
            ("Txs", format_count(model.recent_transactions_count())),
            ("Tx/s", format!("{transaction_rate:.2}")),
        ]),
        model.interaction_mode,
    );

    if let Some(tip) = &model.tip {
        render_card(
            frame,
            cards[2],
            "Local Tip",
            aligned_pair_lines(vec![
                ("Hash", tip.header_hash.chars().take(12).collect()),
                ("Slot", format_slot_ratio(tip.slot, model.startup.target_slot())),
                ("Height", format_count(tip.block_height)),
                (
                    "Adopted",
                    model
                        .last_block_elapsed(now)
                        .map(|duration| format!("{} ago", format_duration(duration)))
                        .unwrap_or_else(|| "—".into()),
                ),
            ]),
            model.interaction_mode,
        );
    }

    render_card(
        frame,
        cards[3],
        "Chain quality",
        aligned_pair_lines(vec![
            ("Chain Growth", "-".into()),
            (
                "Chain Density",
                model
                    .tip
                    .as_ref()
                    .map(|tip| format_density(tip.density, model.startup.active_slot_coeff_inverse))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Rollback depth",
                model
                    .average_recent_rollback_length()
                    .map(|value| if value == 1.0 { "~1 block".into() } else { format!("~{value:.1} blocks") })
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Rollback freq.",
                model.recent_rollback_frequency(now).map(format_secs_frequency).unwrap_or_else(|| "—".into()),
            ),
        ]),
        model.interaction_mode,
    );

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
        .split(layout[2]);
    render_peers_table(frame, bottom[0], model, views, now);
    render_card(
        frame,
        bottom[1],
        "Mempool",
        aligned_pair_lines(vec![
            ("Txs", format_count(model.mempool.tx_count)),
            ("Occupancy", format_kib_ratio(model.mempool.size_bytes, model.startup.mempool_max_bytes)),
        ]),
        model.interaction_mode,
    );
}

pub(in crate::ui) fn page_content_height(model: &Model) -> u16 {
    10 + peers_panel_height(model).max(mempool_panel_height())
}

fn peers_panel_height(model: &Model) -> u16 {
    3 + model.peers.len().min(3) as u16
}

fn mempool_panel_height() -> u16 {
    4
}

fn latest_system_sample(model: &Model) -> Option<&SystemSample> {
    model.system_sample.as_ref()
}

struct GaugeMetric {
    value: Option<String>,
    ratio: f64,
    percent: Option<String>,
}

fn cpu_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    let Some(sample) = sample else {
        return GaugeMetric { value: None, ratio: 0.0, percent: None };
    };

    GaugeMetric {
        value: None,
        ratio: linear_ratio_f64(sample.cpu_percent, 100.0),
        percent: Some(format!("{:.1}%", sample.cpu_percent)),
    }
}

fn disk_read_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    disk_gauge(sample, |sample| sample.disk_live_read_bytes, SystemSample::total_live_read_bytes)
}

fn disk_write_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    disk_gauge(sample, |sample| sample.disk_live_write_bytes, SystemSample::total_live_write_bytes)
}

fn disk_gauge(
    sample: Option<&SystemSample>,
    current: impl Fn(&SystemSample) -> u64,
    total: impl Fn(&SystemSample) -> u64,
) -> GaugeMetric {
    let Some(sample) = sample else {
        return GaugeMetric { value: None, ratio: 0.0, percent: None };
    };

    let current = current(sample);
    let total = total(sample);
    let raw_ratio = linear_ratio(current, total);
    GaugeMetric {
        value: Some(format_disk_rate(current)),
        ratio: raw_ratio,
        percent: Some(format!("{:.1}%", raw_ratio * 100.0)),
    }
}

fn bytes_to_kib(bytes: u64) -> u64 {
    bytes.div_ceil(1_024)
}

fn format_disk_rate(bytes: u64) -> String {
    let kib = bytes_to_kib(bytes);
    if kib >= 2_000 {
        format!("{:.1} MiB/s", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KiB/s", format_count(kib))
    }
}

fn linear_ratio(current: u64, max: u64) -> f64 {
    if max == 0 { 0.0 } else { (current as f64 / max as f64).clamp(0.0, 1.0) }
}

fn linear_ratio_f64(current: f64, max: f64) -> f64 {
    if max == 0.0 { 0.0 } else { (current / max).clamp(0.0, 1.0) }
}

fn blocks_per_second(model: &Model) -> f64 {
    model.blocks_per_second()
}

fn transactions_per_second(model: &Model) -> f64 {
    model.transactions_per_second()
}

fn format_kib(bytes: u64) -> String {
    format!("{} KiB", format_count(bytes.div_ceil(1_024)))
}

fn format_kib_ratio(bytes: u64, capacity_bytes: u64) -> String {
    format!("{} / {}", format_kib(bytes), format_kib(capacity_bytes))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn linear_ratio_keeps_bounds() {
        assert_eq!(linear_ratio(0, 1_000), 0.0);
        assert_eq!(linear_ratio(1_000, 1_000), 1.0);
        assert_eq!(linear_ratio(42, 300), 0.14);
    }

    #[test]
    fn throughput_uses_exponential_moving_average() {
        let mut model = Model::new(
            crate::Config::default(),
            crate::startup::StartupContext::new(
                42,
                "preview",
                "test",
                "test",
                180_224,
                &amaru_kernel::PREVIEW_GLOBAL_PARAMETERS,
                None,
                None,
                Vec::default(),
            ),
        );
        model.block_rate.record(model.created_at + Duration::from_secs(1), 1);
        model.block_rate.record(model.created_at + Duration::from_secs(4), 1);
        model.transaction_rate.record(model.created_at + Duration::from_secs(2), 9);
        model.transaction_rate.record(model.created_at + Duration::from_secs(4), 3);

        assert_eq!(blocks_per_second(&model), 1.0 / 3.0);
        assert_eq!(transactions_per_second(&model), 1.5);
    }

    #[test]
    fn disk_rate_switches_to_mib_per_second_above_threshold() {
        assert_eq!(format_disk_rate(1_999 * 1_024), "1,999 KiB/s");
        assert_eq!(format_disk_rate(2_000 * 1_024), "2.0 MiB/s");
    }
}
